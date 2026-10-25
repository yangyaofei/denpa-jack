// 公共域(阶段A 第5步从 main.ts 拆出): 配置加载/保存、轮询、试音、权限、版本、mock。
// 跨页共享的状态与方法都住这里; 页域模块只管自己页面的编辑态。
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { marked } from "marked";
import type { AppThis, HistoryRecord } from "./types";

export function common(this: AppThis) {
  return {
    section: "history",
    search: "",
    recording: false,
    error: "",
    cfg: null as AppThis["cfg"],
    /** 保存前快照的 data_dir: saveCfg 比对, 变了提示重启生效(D10——后端缓存不中途切换) */
    _prevDataDir: "" as string,
    // 权限总检查(麦克风 + 辅助功能): 启动即查, 缺项顶部横幅 + 通用页卡片
    perms: [] as any[],
    // 应用版本(关于页显示): 从 tauri.conf.json 的 version 读, 避免页面上硬编码后与发布版本漂移
    version: "",
    prioSel: 0,
    mics: [] as { uid: string; name: string }[],
    activeMic: null as [string, string] | null,
    micToAdd: "",

    sections: [
      { id: "history", title: "历史", keywords: "历史 记录 录音 播放 重转 复制" },
      { id: "dict", title: "词典", keywords: "词典 纠错 变体 护栏 权重" },
      { id: "general", title: "通用", keywords: "通用 快捷键 试音 开关" },
      { id: "asr", title: "语音识别", keywords: "asr 档案 麦克风 热词 引擎 key 设备" },
      { id: "llm", title: "大模型", keywords: "llm 润色 档案 提示词 deepseek 智谱" },
      { id: "about", title: "关于", keywords: "关于 版本" },
    ],

    async init(this: AppThis) {
      const _w = window as any;
      if (_w.__denpaInited) return; // 幂等: 只初始化一次(同窗口重复实例不再重复起轮询)
      _w.__denpaInited = true;
      const h = location.hash.slice(1);
      if (["general","asr","dict","llm","history","about"].includes(h)) this.section = h;
      window.addEventListener("hashchange", () => {
        const sec = location.hash.slice(1);
        if (["general","asr","dict","llm","history","about"].includes(sec)) this.section = sec;
      });
      const hasTauri = !!(window as any).__TAURI_INTERNALS__;
      if (!hasTauri) return this.initMock();
      listen("config-changed", async () => {
        const keepSection = this.section;
        await this.loadCfg();
        this.section = keepSection;
      });
      await this.loadCfg();
      await this.refreshHistory();
      await this.refreshMics();
      // 本地网关状态轮询(语音识别页展示; 2s 一次带日志尾部, starting 态靠它走秒)
      this.pollLocalSvc();
      setInterval(() => this.pollLocalSvc(), 2000);
      try {
        this.version = await invoke<string>("app_version");
      } catch (e) {
        console.warn("取应用版本失败", e);
        this.version = "";
      }
      // 录音开始后刷新"当前输入设备"(运行时事实);
      try { this.hotwordsPreview = await invoke("get_hotwords"); } catch (_) {}
      await this.checkPerms();
      this.uiSelftest();
      // 启动时自动检查更新(只报告, 不自动安装); 失败静默
      if (this.cfg?.auto_check_update) this.checkUpdate(true);
      listen("update-progress", (e: any) => {
        const p = e.payload || {};
        const mb = (n: number) => (n / 1024 / 1024).toFixed(1);
        this.updateProgress = p.total
          ? `下载中 ${mb(p.downloaded)} / ${mb(p.total)} MB`
          : `下载中 ${mb(p.downloaded)} MB`;
      });

      // C46 主窗轮询(组件作用域 this 可用): 800ms 拉版本, 变化才刷新
      let lastHistVer = -1;
      let lastCfgVer = -1;
      let lastMicVer = -1;
      let lastPollRecording: { v: boolean | null } = { v: null }; // 后端录音值, 仅后端值变化才同步到按钮
      let tick = 0;
      setInterval(async () => {
        try {
          tick++;
          const v = await invoke<any>("poll_versions");
          // 录音状态同步: 热键/托盘录音 → 主窗试音按钮跟随(core 事实投影)。
          // 只在后端值自身变化时写入——试音点击的乐观置位不被 800ms 轮询覆盖
          // (ctrl_start 前置链冷启动慢, core 置位前乐观值是唯一正确显示)
          if (typeof v.recording === "boolean" && v.recording !== lastPollRecording.v) {
            lastPollRecording.v = v.recording;
            this.recording = v.recording;
          }
          if (tick % 10 === 1) invoke("ui_log", { msg: `poll alive histVer=${v.history}` }).catch(() => {});
          if (v.history !== lastHistVer) {
            const from = lastHistVer;
            lastHistVer = v.history;
            await this.refreshHistory();
            invoke("ui_log", { msg: `history ${from}→${v.history} 刷新完成 ${this.history.length} 条` }).catch(() => {});
          }
          if (v.config !== lastCfgVer) {
            lastCfgVer = v.config;
            await this.loadCfg();
          }
          if (v.mics !== lastMicVer) {
            lastMicVer = v.mics;
            await this.refreshMics();
            invoke("ui_log", { msg: `mics →${v.mics} 重扫完成 ${this.mics.length} 个设备` }).catch(() => {});
          }
        } catch (e) {
          invoke("ui_log", { msg: `poll 异常: ${e}` }).catch(() => {});
        }
      }, 800);
    },

    async loadCfg(this: AppThis) {
      try {
        this.cfg = await invoke("get_config");
        this.axPasteOnlyText = (this.cfg.ax_paste_only_apps ?? []).join("\n");
        this._prevDataDir = this.cfg.data_dir ?? "";
        if (typeof this.refreshHotwords === "function") this.refreshHotwords();
      } catch (e) { this.error = String(e); }
    },
    async saveCfg(this: AppThis) {
      if (!this.cfg) return;
      // 文本域(每行一个 bundle id) → 数组: 去空白行/去重, 顺序保留
      this.cfg.ax_paste_only_apps = Array.from(
        new Set(this.axPasteOnlyText.split("\n").map((s) => s.trim()).filter((s) => s.length > 0)),
      );
      try { await invoke("save_config", { config: this.cfg });
      // D10: 数据目录变更不中途切换(app.log/历史写到一半分叉), 重启后生效
      if ((this.cfg.data_dir ?? "") !== this._prevDataDir) {
        alert("数据目录已更改：将在重启应用后生效，当前会话仍使用原目录。");
        this._prevDataDir = this.cfg.data_dir ?? "";
      }
      try { this.activeMic = await invoke<[string, string] | null>("get_active_mic"); } catch {} }
      catch (e) { this.error = String(e); }
    },

    async toggleRecording(this: AppThis) {
      if (this.recording) {
        this.recording = false;
        try { await invoke("recording_stop"); } catch (e: any) { this.error = String(e); }
      } else {
        this.error = "";
        try { await invoke("recording_start"); this.recording = true; }
        catch (e: any) { this.error = String(e); }
      }
    },

    switchSection(this: AppThis, id: string) {
      this.section = id;
      if (id === "asr" || id === "general") this.refreshMics();
    },
    // 顶栏状态条: 当前组合一眼可见
    statusLine(this: AppThis): string {
      if (!this.cfg) return "加载中…";
      const asr = this.cfg.asr_profiles.find((p: any) => p.id === this.cfg.active_asr_id)?.name ?? "未设置";
      const llm = this.cfg.active_llm_id
        ? (this.cfg.llm_profiles.find((p: any) => p.id === this.cfg.active_llm_id)?.name ?? "关")
        : "关";
      const mic = this.activeMicName();
      const hk = this.hotkeyLabel() || "F5";
      return `${asr} · ${mic} · 按住 ${hk}`;
    },
    activeMicName(this: AppThis): string {
      return this.activeMic?.name ?? "自动(默认麦克风)";
    },
    // 重扫设备列表 + 当前输入设备(事件驱动: CoreAudio 设备变化 → poll_versions.mics 变化时调用)
    async refreshMics(this: AppThis) {
      try {
        const raw = await invoke<[string, string][]>("list_mics");
        this.mics = raw.map(([uid, name]) => ({ uid, name }));
      } catch (e) { console.error("list_mics:", e); }
      try { this.activeMic = await invoke<[string, string] | null>("get_active_mic"); } catch {}
    },
    micName(this: AppThis, uid: string): string {
      return this.mics.find((m: any) => m.uid === uid)?.name ?? uid;
    },
    filteredSections(this: AppThis) {
      const q = this.search.trim().toLowerCase();
      if (!q) return this.sections;
      return this.sections.filter((s: any) => s.title.includes(q) || s.keywords.includes(q));
    },
    mdRender(this: AppThis, src: string): string {
      try {
        return marked.parse(src, { async: false }) as string;
      } catch {
        return src;
      }
    },
    async openConfigFile(this: AppThis) {
      try { await invoke("open_config_file") } catch (e) { this.error = String(e) }
    },
    async toggleAutostart(this: AppThis) {
      try {
        if (this.autostartOn) { await invoke("autostart_disable"); this.autostartOn = false; }
        else { await invoke("autostart_enable"); this.autostartOn = true; }
      } catch (e) { this.error = String(e); }
    },
    async openDataDir(this: AppThis) {
      try { await invoke("open_data_dir"); } catch (e) { console.error(e); }
    },

    // 启动自检: 逐个调用只读命令, 命令失败/异常经 ui_log 落盘(app.log 可查)——交付前必看
    async uiSelftest(this: AppThis) {
      const checks: [string, () => Promise<unknown>][] = [
        ["ping", () => invoke("ping")],
        ["get_config", () => invoke("get_config")],
        ["list_mics", () => invoke("list_mics")],
        ["get_active_mic", () => invoke("get_active_mic")],
        ["get_history", () => invoke("get_history", { limit: 5 })],
        ["get_hotwords", () => invoke("get_hotwords")],
        ["list_llm_models", () => invoke("list_llm_models", { provider: "deepseek", baseUrl: "", apiKey: "" })],
      ];
      for (const [name, fn] of checks) {
        try {
          await fn();
          try { await invoke("ui_log", { msg: `selftest ${name}: OK` }); } catch {}
        } catch (e) {
          const msg = `selftest ${name}: FAIL ${String(e).slice(0, 120)}`;
          console.error(msg);
          try { await invoke("ui_log", { msg }); } catch {}
        }
      }
    },

    // ==== 权限总检查(必需: 麦克风 + 辅助功能; 可选: 屏幕录制) ====
    // 只把必需项缺失当作问题(顶部横幅/启动提示只看这个)
    missingPerms(this: AppThis): any[] {
      return this.perms.filter((p: any) => p.required !== false && !p.granted);
    },
    permStatusText(this: AppThis, p: any): string {
      if (p.granted) return "已授权";
      if (p.status === "not_determined") return "未决定（首次使用会弹窗）";
      if (p.status === "restricted") return "受限";
      return "未授权";
    },
    async checkPerms(this: AppThis) {
      try {
        this.perms = await invoke<any[]>("check_permissions");
      } catch (e) {
        console.warn("check_permissions 失败", e);
      }
    },
    async requestMic(this: AppThis) {
      try { await invoke("request_microphone"); } catch (e) { console.warn(e); }
      setTimeout(() => this.checkPerms(), 900);
    },
    async requestScreenPerm(this: AppThis) {
      try { await invoke("request_screen_permission"); } catch (e) { console.warn(e); }
      setTimeout(() => this.checkPerms(), 900);
    },
    async openPermSettings(this: AppThis, p: any) {
      try { await invoke("open_system_settings", { url: p.settings_url }); } catch (e) { console.warn(e); }
    },

    // headless 自测 mock: 无 Tauri 环境时提供示例数据
    async initMock(this: AppThis) {
      this.mics = [
          { uid: "mock-uid-1", name: "Wireless Mic Rx (DJI)" },
          { uid: "mock-uid-2", name: "MacBook Pro麦克风" },
        ];
      this.hotwordsPreview = ["谢克数学", "腾讯云"];
      this.cfg = {
        keys: ["sk-demo"], llm_profiles: [
          { id: "l1", name: "DeepSeek 润色", provider: "deepseek", base_url: "", model: "deepseek-chat", api_key: "sk-demo", prompt: "", thinking: false, effort: "low" },
          { id: "l2", name: "智谱 Flash", provider: "zhipu", base_url: "", model: "glm-5.3-flash", api_key: "", prompt: "", thinking: true, effort: "low" },
        ], asr_profiles: [
          { id: "a1", name: "豆包流式", provider: "volcengine", api_key: "" },
        ], dict: [
          { term: "谢克数学", variants: ["些克数学", "歇课数学"], guard_words: [], boost: 3 },
          { term: "腾讯云", variants: ["腾讯营"], guard_words: [], boost: 2 },
        ], normalizations: [], active_llm_id: "l1", active_asr_id: "a1",
        use_llm_correction: false, clipboard_only: false,
        activation: "hold", audio_feedback: true, restore_clipboard: true,
        keep_in_clipboard: false, trailing_newlines: 0, ax_paste_only_apps: [], local_autostart: true, local_service_dir: "~/workspace/denpa-asr",
        builtin_engine: { model: "Qwen/Qwen3-ASR-1.7B", vad: "", twopass: true, chunk_size_sec: 0.5, max_context_sec: 120, endpointing: "energy", max_sessions: 4 },
        overlay_position: "bottom", history_limit: 200,
        max_recording_seconds: 1800, min_recording_seconds: 0.3, keep_audio_count: 50,
        llm_timeout_secs: 30, llm_max_tokens: 0, llm_retries: 2,
        auto_check_update: true,
        extra_tail_ms: 0, mic_device_uid: "", mic_priority: ["Wireless Mic Rx (DJI)"],
        data_dir: "",
        bindings: { transcribe: { current: ["f5"] } },
      } as any;
      this.autostartOn = false;
      this.history = [
        { ts: "2026-09-02 13:00:01", engine: "volcengine", raw: "歇课数学很厉害", final_text: "谢克数学很厉害", llm_used: true, delivered: "clipboard+paste", audio_path: "" },
        { ts: "2026-09-02 12:58:10", engine: "volcengine", raw: "我们在腾讯营上部署", final_text: "我们在腾讯云上部署", llm_used: false, delivered: "clipboard", audio_path: "" },
      ] as HistoryRecord[];
    },
  };
}
