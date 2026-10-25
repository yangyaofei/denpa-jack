// ASR 页域(阶段A 第5步从 main.ts 拆出): 引擎档案增删改用 + 内置网关管理。
import { invoke } from "@tauri-apps/api/core";
import { uid } from "./util";
import type { AppThis, AsrProfile } from "./types";

export function asrPage(this: AppThis) {
  return {
    asrEdit: null as AsrProfile | null,
    asrIsNew: false,
    builtinDirty: false as boolean,
    localSvc: { state: "unknown" as string, port: 0 as number, elapsed_secs: null as number | null, exit: null as string | null, tail: [] as string[], busy: false },

    // ── ASR 档案 ──
    newAsr(this: AppThis) {
      this.asrIsNew = true;
      this.asrEdit = { id: uid(), name: "新引擎", provider: "volcengine", api_key: "", base_url: "", hotwords_enabled: true };
    },
    editAsr(this: AppThis, p: AsrProfile) { this.asrIsNew = false; this.asrEdit = JSON.parse(JSON.stringify(p)); },
    async delAsr(this: AppThis, p: AsrProfile) {
      if (!this.cfg) return;
      this.cfg.asr_profiles = this.cfg.asr_profiles.filter((x) => x.id !== p.id);
      if (this.cfg.active_asr_id === p.id) this.cfg.active_asr_id = this.cfg.asr_profiles[0]?.id ?? "";
      await this.saveCfg();
    },
    async useAsr(this: AppThis, p: AsrProfile | { id: string }) {
      if (!this.cfg) return;
      this.cfg.active_asr_id = p.id;
      await this.saveCfg();
      // 切到内置引擎且未运行 → 立即拉起(不等下次 APP 启动的预热)
      if (p.id === "builtin" && this.localSvc.state !== "running" && this.localSvc.state !== "starting") {
        this.localSvcAction("start");
      }
    },
    markBuiltinDirty(this: AppThis) {
      this.builtinDirty = true;
    },
    // 配置改动不立即重启(不打断进行中的录音), 用户点"重启生效": 停止→等落定→按新参数拉起
    async restartBuiltin(this: AppThis) {
      this.localSvc.busy = true;
      try {
        await invoke("local_service_stop");
        for (let i = 0; i < 20; i++) {
          await new Promise((r) => setTimeout(r, 300));
          const s = await invoke<any>("local_service_status");
          if (s.state === "stopped" || s.state === "failed" || s.state === "crashed") break;
        }
        await invoke("local_service_start");
        this.builtinDirty = false;
        this.pollLocalSvc();
      } finally {
        this.localSvc.busy = false;
      }
    },
    // ── 本地网关(内置引擎子进程) ──
    async pollLocalSvc(this: AppThis) {
      try {
        const s = await invoke<any>("local_service_status");
        // starting 态保留本地时钟计时(status 的 elapsed 是拉起时刻起的, 已含)
        this.localSvc = { ...this.localSvc, ...s };
        const el = document.getElementById("local-svc-log");
        if (el) el.scrollTop = el.scrollHeight; // 日志自动滚到底(等效 tail -f)
      } catch { /* 探活失败按未知处理, 不打扰 */ }
    },
    async localSvcAction(this: AppThis, act: "start" | "stop") {
      this.localSvc.busy = true;
      try {
        const r = await invoke<string>(act === "start" ? "local_service_start" : "local_service_stop");
        if (r === "not-ours") alert("这个实例不是 APP 拉起的（手动 serve），请自行停止。");
        await this.pollLocalSvc();
      } catch (e) {
        alert(`操作失败: ${e}`);
      } finally {
        this.localSvc.busy = false;
      }
    },
    get localSvcText(this: AppThis): string {
      const s = this.localSvc.state;
      if (s === "running") return "运行中";
      if (s === "starting")
        return `启动中… ${this.localSvc.elapsed_secs ?? 0}s（模型加载约 40s，日志见下）`;
      if (s === "failed") return `启动失败（退出: ${this.localSvc.exit ?? "?"}，看下方日志）`;
      if (s === "crashed") return `运行中崩溃（退出: ${this.localSvc.exit ?? "?"}，可重新启动）`;
      return "未运行";
    },

    async saveAsr(this: AppThis) {
      if (!this.cfg || !this.asrEdit) return;
      // Key 回写全局池
      if (this.asrEdit.api_key.trim() && !this.cfg.keys.includes(this.asrEdit.api_key.trim())) {
        this.cfg.keys.push(this.asrEdit.api_key.trim());
      }
      const i = this.cfg.asr_profiles.findIndex((x) => x.id === this.asrEdit!.id);
      if (i >= 0) this.cfg.asr_profiles[i] = this.asrEdit;
      else this.cfg.asr_profiles.push(this.asrEdit);
      if (!this.cfg.active_asr_id) this.cfg.active_asr_id = this.asrEdit.id;
      this.asrEdit = null;
      await this.saveCfg();
    },
    async switchAsr(this: AppThis, ev: Event) {
      if (!this.cfg) return;
      this.cfg.active_asr_id = (ev.target as HTMLSelectElement).value;
      await this.saveCfg();
    },
  };
}
