import "./style.css";
import pageAsr from "./pages/asr.html?raw";
import pageHistory from "./pages/history.html?raw";
import pageGeneral from "./pages/general.html?raw";
import pageDict from "./pages/dict.html?raw";
import pageLlm from "./pages/llm.html?raw";
import pageAbout from "./pages/about.html?raw";

import Alpine from "alpinejs";
import { invoke, Channel } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { marked } from "marked";

// ── 与 Rust config.rs 对应的类型 ──
export interface LlmProfile {
  id: string; name: string; provider: string; model: string;
  api_key: string; prompt: string; thinking: boolean;
}
export interface AsrProfile {
  id: string; name: string; provider: string; api_key: string; hotwords_enabled: boolean;
}
export interface DictEntry {
  term: string; variants: string[]; guard_words: string[]; boost: number;
}
export interface RegexRule { pattern: string; replacement: string; }
export interface HotkeyConfig {
  key: string; ctrl: boolean; alt: boolean; cmd: boolean; shift: boolean;
}
export interface Config {
  keys: string[]; llm_profiles: LlmProfile[]; asr_profiles: AsrProfile[];
  dict: DictEntry[]; normalizations: RegexRule[];
  active_llm_id: string; active_asr_id: string;
  hotkey_key_code: number; use_llm_correction: boolean; clipboard_only: boolean;
  shortcut_activation: string; hold_threshold_ms: number; audio_feedback: boolean; restore_clipboard: boolean;
  filler_word_removal: boolean; append_trailing_space: boolean; auto_submit: boolean;
  overlay_position: string; history_limit: number;
  max_recording_seconds: number; min_recording_seconds: number; keep_audio_count: number;
  mic_device_uid: string; mic_priority: string[];
  hotkey: HotkeyConfig;
}
export interface HistoryRecord {
  ts: string; engine: string; raw: string; final_text: string;
  llm_used: boolean; delivered: string; audio_path: string; warning?: string;
}

const uid = () => Math.random().toString(36).slice(2, 10);

// event.code → global-hotkey Code 格式(keyboard_types: KeyA/Space/F5/Digit1/Comma)
function codeToKey(code: string): string | null {
  if (code === "Escape") return null;
  if (/^(F[1-9]|F1[0-2])$/.test(code)) return code;
  if (code.startsWith("Key") || code.startsWith("Digit") || code.startsWith("Arrow") || code.startsWith("Numpad")) return code;
  const map: Record<string, string> = {
    Space: "Space", Enter: "Enter", Tab: "Tab", Backspace: "Backspace",
    Comma: "Comma", Period: "Period", Slash: "Slash", Semicolon: "Semicolon",
    Quote: "Quote", Minus: "Minus", Equal: "Equal",
    BracketLeft: "BracketLeft", BracketRight: "BracketRight", Backslash: "Backslash",
    IntlBackslash: "IntlBackslash", CapsLock: "CapsLock",
  };
  return map[code] ?? null;
}

/// C54: 组合字符串美化("ctrl+cmd+space" → "⌃⌘ Space")
function describeCombo(s: string): string {
  const sym: Record<string, string> = { ctrl: "⌃", option: "⌥", alt: "⌥", shift: "⇧", cmd: "⌘", fn: "Fn" };
  const keyNames: Record<string, string> = {
    space: "Space", enter: "Return", backspace: "Delete", tab: "Tab", esc: "Esc",
    left: "←", right: "→", up: "↑", down: "↓", "`": "`", "-": "-", "=": "=",
    "[": "[", "]": "]", "\\": "\\", ";": ";", "'": "'", ",": ",", ".": ".", "/": "/",
  };
  const parts = s.split("+").map((p) => {
    const lower = p.toLowerCase();
    if (sym[lower]) return sym[lower];
    if (keyNames[lower]) return keyNames[lower];
    if (/^f\d{1,2}$/.test(lower)) return lower.toUpperCase();
    return p.length <= 1 ? p.toUpperCase() : p.charAt(0).toUpperCase() + p.slice(1);
  });
  // 修饰符号直接连排, 与主键之间留空格
  const modSyms = parts.filter((p) => /^[⌃⌥⇧⌘]$/.test(p) || p === "Fn");
  const rest = parts.filter((p) => !modSyms.includes(p) || p === "Fn");
  const main = rest.filter((p) => p !== "Fn");
  if (!main.length) return modSyms.join(" ");
  return modSyms.join("") + " " + main.join(" ");
}

function describeHotkey(h: HotkeyConfig): string {
  const parts: string[] = [];
  if (h.ctrl) parts.push("Ctrl");
  if (h.alt) parts.push("Opt");
  if (h.shift) parts.push("Shift");
  if (h.cmd) parts.push("Cmd");
  const keyNames: Record<string, string> = {
    Space: "Space", Enter: "Return", Backspace: "Delete", Tab: "Tab",
    ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→",
    Comma: ",", Period: ".", Slash: "/", Semicolon: ";", Quote: "'",
    Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]",
  };
  const key = h.key.charAt(0).toUpperCase() + h.key.slice(1);
  parts.push(keyNames[key] ?? keyNames[h.key] ?? key.replace(/^Key/, ""));
  return parts.join(" + ");
}

// app() 全局 Alpine 组件
(window as any).app = () => ({
  section: "history",
  search: "",
  recording: false,
  partial: "",
  result: "",
  error: "",
  finalInfo: null as any,
  mics: [] as { uid: string; name: string }[],
    activeMic: null as [string, string] | null,
    micToAdd: "",
  cfg: null as Config | null,
  history: [] as HistoryRecord[],
  selHist: null as HistoryRecord | null,
  asrEdit: null as AsrProfile | null,
  asrIsNew: false,
  llmEdit: null as LlmProfile | null,
  llmIsNew: false,
  dictEdit: null as any,
  dictIsNew: false,
  keysText: "",
    hotwordsPreview: [] as string[],
  dictSearch: "",
  hotkeyHint: "",
  hkCapturing: false,
  hkCurrent: "",
  hkFinal: "",
    llmModels: [] as string[],
    selftesting: false,
    selftestResult: "",
    editLlmPromptPreview: false,
    defaultPrompt: "",
    histSearch: "",
    quickTerm: "",
    settingsOpen: false,
    llmModelsError: "",
  permWarning: "",
  prioSel: 0,

  sections: [
    { id: "history", title: "历史", keywords: "历史 记录 录音 播放 重转 复制" },
    { id: "dict", title: "词典", keywords: "词典 纠错 变体 护栏 权重" },
    { id: "general", title: "通用", keywords: "通用 快捷键 试音 开关" },
    { id: "asr", title: "语音识别", keywords: "asr 档案 麦克风 热词 引擎 key 设备" },
    { id: "llm", title: "大模型", keywords: "llm 润色 档案 提示词 deepseek 智谱" },
    { id: "about", title: "关于", keywords: "关于 版本" },
  ],

  async init() {
    const h = location.hash.slice(1);
    if (["general","asr","dict","llm","history","about"].includes(h)) this.section = h;
    window.addEventListener("hashchange", () => {
      const sec = location.hash.slice(1);
      if (["general","asr","dict","llm","history","about"].includes(sec)) this.section = sec;
    });
    const hasTauri = !!(window as any).__TAURI_INTERNALS__;
    if (!hasTauri) return this.initMock();;;;;;;
    listen("config-changed", async () => {
      const keepSection = this.section;
      await this.loadCfg();
      this.section = keepSection;
    });
    await this.loadCfg();
    await this.refreshHistory();
    try {
      // Rust Vec<(uid,name)> 序列化为数组的数组 → 转对象
      const raw = await invoke<[string, string][]>("list_mics");
      this.mics = raw.map(([uid, name]) => ({ uid, name }));
    } catch (e) { console.error("list_mics:", e); }
    try { this.activeMic = await invoke<[string, string] | null>("get_active_mic"); } catch {}
    // 录音开始后刷新"当前输入设备"(运行时事实);
    try { this.hotwordsPreview = await invoke("get_hotwords"); } catch (_) {}
    invoke("check_permissions").catch(() => {});
    this.uiSelftest();
  
    // C46 主窗轮询(组件作用域 this 可用): 800ms 拉版本, 变化才刷新
    let lastHistVer = -1;
    let lastCfgVer = -1;
    let tick = 0;
    setInterval(async () => {
      try {
        tick++;
        const v = await invoke<any>("poll_versions");
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
      } catch (e) {
        invoke("ui_log", { msg: `poll 异常: ${e}` }).catch(() => {});
      }
    }, 800);
  },

  async loadCfg() {
    try {
      this.cfg = await invoke("get_config");
      this.keysText = this.cfg.keys.join("\n");
      // C51f: shortcut_str 是 Rust 方法不在 JSON——前端重建行文本
      this.extraHotkeys = (this.cfg.hotkeys || [])
        .map((h: any) => [h.ctrl && "ctrl", h.alt && "alt", h.shift && "shift", h.cmd && "cmd", h.key].filter(Boolean).join("+"))
        .join("\n");
      if (typeof this.refreshHotwords === "function") this.refreshHotwords();
    } catch (e) { this.error = String(e); }
  },
  async saveCfg() {
    if (!this.cfg) return;
    this.cfg.keys = this.keysText.split("\n").map((s: string) => s.trim()).filter(Boolean);
    try { await invoke("save_config", { config: this.cfg });
    try { this.activeMic = await invoke<[string, string] | null>("get_active_mic"); } catch {} }
    catch (e) { this.error = String(e); }
  },

  async toggleRecording() {
    if (this.recording) {
      this.recording = false;
      try { await invoke("recording_stop"); } catch (e: any) { this.error = String(e); }
    } else {
      this.partial = ""; this.result = ""; this.error = ""; this.finalInfo = null;
      try { await invoke("recording_start"); this.recording = true; }
      catch (e: any) { this.error = String(e); }
    }
  },

  // ── 快捷键录制 ──
  stars(boost: number): string {
    return "★★★".slice(0, Math.max(0, boost)) + "☆☆☆".slice(0, Math.max(0, 3 - boost));
  },
  weightStars(b: number): string { return this.stars(b); },
  describeHotkey,
  describeCombo,
  hotkeyLabel() {
    // C54: 顶部文案取 bindings 列表第一个(迁移后唯一真相)
    const first = this.cfg?.bindings?.["transcribe"]?.current?.[0];
    return first ? describeCombo(first) : "";
  },

  // C54: 绑定列表(bindings.transcribe.current 字符串数组, 唯一真相)
  hkList(): string[] {
    return this.cfg?.bindings?.["transcribe"]?.current ?? [];
  },
  async removeHotkey(combo: string) {
    try {
      await invoke("remove_binding", { combo });
      await this.loadCfg();
    } catch (x: any) { this.error = String(x); }
  },
  // C54 前端录制(Handy GlobalShortcutInput 同构): suspend 真注销 → window keydown/keyup
  // 收集组合(修饰排序在前) → 全抬即 add_binding 提交 → resume 恢复。无后端 capture 线程。
  _hkPressed: [] as string[],
  _hkRecorded: [] as string[],
  _hkCleanup: null as (() => void) | null,
  async startHotkeyRecord() {
    if (this.hkCapturing || !this.cfg) return;
    try { await invoke("suspend_all_bindings"); } catch (x: any) { this.error = String(x); return; }
    this.hkCapturing = true;
    this.hkCurrent = "";
    this.hkFinal = "";
    this.hotkeyHint = "";
    const MODS = ["control", "ctrl", "shift", "alt", "option", "meta", "command", "cmd"];
    const normKey = (e: KeyboardEvent): string => {
      if (e.key === "Control") return "ctrl";
      if (e.key === "Shift") return "shift";
      if (e.key === "Alt" || e.key === "Option") return "option";
      if (e.key === "Meta" || e.key === "Command") return "cmd";
      if (e.code?.startsWith("Key")) return e.code.slice(3).toLowerCase();
      if (e.code?.startsWith("Digit")) return e.code.slice(5);
      if (e.code?.startsWith("Numpad")) return e.code.toLowerCase();
      const map: Record<string, string> = {
        " ": "space", Spacebar: "space", Enter: "enter", Tab: "tab", Backspace: "backspace",
        ArrowLeft: "left", ArrowRight: "right", ArrowUp: "up", ArrowDown: "down",
        Escape: "esc", "`": "`", "-": "-", "=": "=", "[": "[", "]": "]", "\\": "\\", ";": ";",
        "'": "'", ",": ",", ".": ".", "/": "/",
      };
      if (map[e.key] !== undefined) return map[e.key];
      if (/^F\d{1,2}$/.test(e.key)) return e.key.toLowerCase();
      return e.key.toLowerCase();
    };
    const kd = (e: KeyboardEvent) => {
      if (e.repeat) { e.preventDefault(); return; }
      e.preventDefault();
      const k = normKey(e);
      if (!this._hkPressed.includes(k)) {
        this._hkPressed.push(k);
        if (!this._hkRecorded.includes(k)) this._hkRecorded.push(k);
      }
      this.hkCurrent = this._hkRecorded.join("+");
    };
    const finish = async () => {
      const recorded = [...this._hkRecorded];
      const pressed = [...this._hkPressed];
      if (pressed.length === 0 && recorded.length > 0) {
        // 全部抬起 → 提交(修饰排序在前)
        const sorted = recorded.sort((a, b) => {
          const am = MODS.includes(a), bm = MODS.includes(b);
          if (am && !bm) return -1;
          if (!am && bm) return 1;
          return 0;
        });
        const combo = sorted.join("+");
        this.hkFinal = combo;
        try {
          await invoke("add_binding", { combo });
          await invoke("resume_all_bindings");
          this.hkCapturing = false; this.hkFinal = ""; this.hotkeyHint = "";
          await this.loadCfg();
        } catch (x: any) {
          this.error = String(x);
          this.hkCapturing = false;
          invoke("resume_all_bindings").catch(() => {});
        }
      }
    };
    const ku = (e: KeyboardEvent) => {
      e.preventDefault();
      const k = normKey(e);
      this._hkPressed = this._hkPressed.filter((x) => x !== k);
      finish();
    };
    const cancel = async () => {
      this._hkPressed = []; this._hkRecorded = [];
      this.hkCapturing = false; this.hkCurrent = ""; this.hkFinal = ""; this.hotkeyHint = "";
      invoke("resume_all_bindings").catch(() => {});
    };
    window.addEventListener("keydown", kd, true);
    window.addEventListener("keyup", ku, true);
    this._hkCleanup = () => {
      window.removeEventListener("keydown", kd, true);
      window.removeEventListener("keyup", ku, true);
    };
    // Esc 取消录制
    const escWatch = (e: KeyboardEvent) => { if (e.key === "Escape") { this._hkCleanup?.(); this._hkCleanup = null; window.removeEventListener("keydown", escWatch, true); cancel(); } };
    window.addEventListener("keydown", escWatch, true);
    const origCleanup = this._hkCleanup;
    this._hkCleanup = () => { origCleanup(); window.removeEventListener("keydown", escWatch, true); };
  },
  async cancelHotkey() {
    this._hkCleanup?.();
    this._hkCleanup = null;
    this._hkPressed = []; this._hkRecorded = [];
    this.hkCapturing = false; this.hkCurrent = ""; this.hkFinal = ""; this.hotkeyHint = "";
    invoke("resume_all_bindings").catch(() => {});
  },
  async addFnPreset(preset: string) {
    try {
      await invoke("add_binding", { combo: preset });
      await this.loadCfg();
    } catch (x: any) { this.error = String(x); }
  },

  async setMicDevice(uid: string) {
    if (!this.cfg) return;
    this.cfg.mic_device_uid = uid;
    await this.saveCfg();
  },
  mdRender(src: string): string {
    try {
      return marked.parse(src, { async: false }) as string;
    } catch {
      return src;
    }
  },
  async togglePromptPreview() {
    if (this.editLlmPromptPreview) {
      this.editLlmPromptPreview = false;
      return;
    }
    // 预览前确保有内容可渲染
    if (!this.llmEdit?.prompt) {
      try { this.llmEdit.prompt = await invoke<string>("get_default_prompt"); } catch {}
    }
    this.editLlmPromptPreview = true;
  },
  async runLlmSelftest() {
    if (!this.cfg || this.selftesting) return;
    this.selftesting = true;
    this.selftestResult = "";
    try {
      const r = await invoke<any>("llm_selftest", {
        profile: this.llmEdit ? {
          provider: this.llmEdit.provider,
          base_url: this.llmEdit.base_url,
          model: this.llmEdit.model,
          api_key: this.llmEdit.api_key,
          prompt: this.llmEdit.prompt,
          thinking: this.llmEdit.thinking,
        } : null,
      });
      this.selftestResult = [
        `① 词典纠错: ${r.dict_text}`,
        `② LLM(${r.tool_used ? "tool_calls ✓" : "⚠ 未走工具"}) ${r.latency_ms}ms: ${r.llm_text}`,
      ].join("\n");
    } catch (e) {
      this.selftestResult = `✗ 失败: ${String(e)}`;
    }
    this.selftesting = false;
  },
  async fetchLlmModels() {
    try {
      this.llmModels = await invoke<string[]>("list_llm_models", {
        provider: this.llmEdit?.provider ?? "deepseek",
        baseUrl: this.llmEdit?.base_url ?? "",
        apiKey: this.llmEdit?.api_key ?? "",
      });
      this.llmModelsError = "";
      // 默认模型: 列表首个(当前值不在列表时)
      if (this.llmModels.length && !this.llmModels.includes(this.llmEdit?.model ?? "")) {
        this.llmEdit.model = this.llmModels[0];
      }
    } catch (e) {
      this.llmModelsError = String(e);
      this.llmModels = [];
    }
  },
  // 思考强度选项: 每家来自各自官网 enum(现在恰好相同, 分叉时各自改)
  effortOptions(): string[] {
    return this.llmEdit?.provider === "zhipu"
      ? ["max", "high", "low"] // 智谱官网 enum: max/xhigh/high/medium/low/minimal/none, 5.3 系仅后三档
      : ["max", "high", "low"]; // DeepSeek 官网 enum: low/high/max
  },
  onLlmProviderChange() {
    const defaults: Record<string, string> = {
      deepseek: "https://api.deepseek.com",
      zhipu: "https://open.bigmodel.cn/api/paas/v4",
    };
    if (this.llmEdit) {
      this.llmEdit.base_url = defaults[this.llmEdit.provider] ?? "";
      // effort 选项随供应商变化, 值重置为该家官网默认(智谱 max / DeepSeek high)
      const effDefault: Record<string, string> = { zhipu: "max", deepseek: "high" };
      this.llmEdit.effort = effDefault[this.llmEdit.provider] ?? "low";
      this.fetchLlmModels();
    }
  },
  async addToPriority() {
    if (!this.cfg || !this.micToAdd) return;
    if (!this.cfg.mic_priority.includes(this.micToAdd)) {
      this.cfg.mic_priority.push(this.micToAdd);
      await this.saveCfg();
    }
    this.micToAdd = "";
  },
  async movePriority(i: number, dir: -1 | 1) {
    if (!this.cfg) return;
    const j = i + dir;
    if (j < 0 || j >= this.cfg.mic_priority.length) return;
    const arr = this.cfg.mic_priority;
    [arr[i], arr[j]] = [arr[j], arr[i]];
    await this.saveCfg();
  },
  async removePriority(i: number) {
    if (!this.cfg) return;
    this.cfg.mic_priority.splice(i, 1);
    await this.saveCfg();
  },

  // ── ASR 档案 ──
  newAsr() {
    this.asrIsNew = true;
    this.asrEdit = { id: uid(), name: "新引擎", provider: "volcengine", api_key: "", hotwords_enabled: true };
  },
  editAsr(p: AsrProfile) { this.asrIsNew = false; this.asrEdit = JSON.parse(JSON.stringify(p)); },
  async delAsr(p: AsrProfile) {
    if (!this.cfg) return;
    this.cfg.asr_profiles = this.cfg.asr_profiles.filter((x) => x.id !== p.id);
    if (this.cfg.active_asr_id === p.id) this.cfg.active_asr_id = this.cfg.asr_profiles[0]?.id ?? "";
    await this.saveCfg();
  },
  async useAsr(p: AsrProfile) {
    if (!this.cfg) return;
    this.cfg.active_asr_id = p.id;
    await this.saveCfg();
  },
  async saveAsr() {
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

  // ── LLM 档案 ──
  newLlm() {
    this.llmIsNew = true;
    this.llmEdit = { id: uid(), name: "新模型", provider: "deepseek", base_url: "", model: "", api_key: "", prompt: "", thinking: false, effort: "high" };
    this.editLlmPromptPreview = false;
    this.fetchLlmModels();
  },
  async editLlm(p: LlmProfile) {
    this.llmIsNew = false;
    this.llmEdit = JSON.parse(JSON.stringify(p));
    // prompt 空则回填内置默认(直接显示真实将发送的内容)
    if (!this.llmEdit.prompt) {
      try { this.llmEdit.prompt = await invoke<string>("get_default_prompt"); } catch {}
    }
    this.fetchLlmModels();
  },
  async delLlm(p: LlmProfile) {
    if (!this.cfg) return;
    this.cfg.llm_profiles = this.cfg.llm_profiles.filter((x) => x.id !== p.id);
    if (this.cfg.active_llm_id === p.id) this.cfg.active_llm_id = this.cfg.llm_profiles[0]?.id ?? "";
    await this.saveCfg();
  },
  async useLlm(p: LlmProfile) {
    if (!this.cfg) return;
    this.cfg.active_llm_id = p.id;
    await this.saveCfg();
  },
  async saveLlm() {
    if (!this.cfg || !this.llmEdit) return;
    if (this.llmEdit.api_key.trim() && !this.cfg.keys.includes(this.llmEdit.api_key.trim())) {
      this.cfg.keys.push(this.llmEdit.api_key.trim());
    }
    const i = this.cfg.llm_profiles.findIndex((x) => x.id === this.llmEdit!.id);
    if (i >= 0) this.cfg.llm_profiles[i] = this.llmEdit;
    else this.cfg.llm_profiles.push(this.llmEdit);
    if (!this.cfg.active_llm_id) this.cfg.active_llm_id = this.llmEdit.id;
    this.llmEdit = null;
    await this.saveCfg();
  },

  // ── 词典 ──
  filteredDict(): DictEntry[] {
    const d = this.cfg?.dict ?? [];
    const q = this.dictSearch.trim().toLowerCase();
    if (!q) return d;
    return d.filter((e) => e.term.toLowerCase().includes(q) || e.variants.some((v) => v.toLowerCase().includes(q)));
  },
  newDict() {
    this.dictIsNew = true;
    this.dictEdit = { term: "", variants: "", guard_words: "", boost: 2 };
  },
  editDict(e: DictEntry) {
    this.dictIsNew = false;
    this.dictEdit = { term: e.term, variants: e.variants.join(", "), guard_words: e.guard_words.join(", "), boost: e.boost };
  },
  async delDict(e: DictEntry) {
    if (!this.cfg) return;
    this.cfg.dict = this.cfg.dict.filter((x) => x.term !== e.term);
    await this.saveCfg();
    await this.refreshHotwords();
  },
  async saveDict() {
    if (!this.cfg || !this.dictEdit?.term.trim()) return;
    const entry: DictEntry = {
      term: this.dictEdit.term.trim(),
      variants: this.dictEdit.variants.split(/[,，]/).map((s: string) => s.trim()).filter(Boolean),
      guard_words: this.dictEdit.guard_words.split(/[,，]/).map((s: string) => s.trim()).filter(Boolean),
      boost: Number(this.dictEdit.boost) || 0,
    };
    const i = this.cfg.dict.findIndex((x) => x.term === entry.term);
    if (i >= 0) this.cfg.dict[i] = entry; else this.cfg.dict.push(entry);
    this.dictEdit = null;
    await this.saveCfg();
    await this.refreshHotwords();
  },
  async refreshHotwords() {
    try { this.hotwordsPreview = await invoke("get_hotwords"); } catch (_) {}
  },

  // ── 历史 ──
  async refreshHistory() {
    // E: 详情是快照——列表刷新时按 ts 重找, 保证重转/重跑后详情同步
    if (this.selHist) {
      const fresh = (this.history || []).find((h: any) => h.ts === this.selHist.ts);
      if (fresh) this.selHist = fresh;
    }
    try { this.history = await invoke("get_history", { limit: 500 }); }
    catch (e) { console.error("get_history:", e); }
  },
  async copyHist(h: HistoryRecord) {
    try { await invoke("copy_text", { text: h.final_text }); } catch (e) { console.error(e); }
  },
  async openAudio(h: HistoryRecord) {
    if (!h.audio_path) return;
    try {
      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
      await revealItemInDir(h.audio_path);
    } catch (e) { console.error(e); }
  },
  async playHist(h: HistoryRecord) {
    if (!h.audio_path) return;
    try {
      const { openPath } = await import("@tauri-apps/plugin-opener");
      await openPath(h.audio_path);
    } catch (e) { console.error(e); }
  },
  async rerunHist(h: HistoryRecord) {
    if (!h.audio_path) { this.error = "该记录无音频, 无法重转"; return; }
    try {
      await invoke("rerun_history", { audioPath: h.audio_path });
      this.error = "";
    } catch (e: any) { this.error = String(e); }
  },
  clearArm: false,
  async clearHist() {
    if (!this.clearArm) {
      this.clearArm = true
      setTimeout(() => { this.clearArm = false }, 3000)
      return
    }
    if (!confirm("清空全部历史记录?")) return;
    try {
      await invoke("clear_history");
      this.history = [];
      this.selHist = null;
    } catch (e) { console.error(e); }
  },
  async openConfigFile() {
    try { await invoke("open_config_file") } catch (e) { this.error = String(e) }
  },
  async toggleAutostart() {
    try {
      if (this.autostartOn) { await invoke("autostart_disable"); this.autostartOn = false; }
      else { await invoke("autostart_enable"); this.autostartOn = true; }
    } catch (e) { this.error = String(e); }
  },
  async openDataDir() {
    try { await invoke("open_data_dir"); } catch (e) { console.error(e); }
  },

  // headless 自测 mock: 无 Tauri 环境时提供示例数据
  // 启动自检: 逐个调用只读命令, 断链/异常经 ui_log 落盘(app.log 可查)——交付前必看
  async uiSelftest() {
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

  async initMock() {
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
        { id: "a1", name: "豆包流式", provider: "volcengine", api_key: "", hotwords_enabled: true },
      ], dict: [
        { term: "谢克数学", variants: ["些克数学", "歇课数学"], guard_words: [], boost: 3 },
        { term: "腾讯云", variants: ["腾讯营"], guard_words: [], boost: 2 },
      ], normalizations: [], active_llm_id: "l1", active_asr_id: "a1",
      hotkey_key_code: 96, use_llm_correction: false, clipboard_only: false,
      shortcut_activation: "hold_or_toggle", hold_threshold_ms: 300, audio_feedback: true, restore_clipboard: true,
      filler_word_removal: false, append_trailing_space: false, auto_submit: false,
      overlay_position: "bottom", history_limit: 200,
      max_recording_seconds: 1800, min_recording_seconds: 0.3, keep_audio_count: 50,
      mic_device_uid: "", mic_priority: ["Wireless Mic Rx (DJI)"],
      hotkey: { key: "f5", ctrl: false, alt: false, cmd: false, shift: false },
    };
    this.autostartOn = false;
    this.keysText = "sk-demo";
    this.history = [
      { ts: "2026-09-02 13:00:01", engine: "volcengine", raw: "歇课数学很厉害", final_text: "谢克数学很厉害", llm_used: true, delivered: "clipboard+paste", audio_path: "" },
      { ts: "2026-09-02 12:58:10", engine: "volcengine", raw: "我们在腾讯营上部署", final_text: "我们在腾讯云上部署", llm_used: false, delivered: "clipboard", audio_path: "" },
    ];
  },

  // 主界面切换器
  toggleRec() { return this.toggleRecording(); },
  async saveKeys() {
    if (!this.cfg) return;
    this.cfg.keys = this.keysText.split(/[,\n]/).map((x: string) => x.trim()).filter(Boolean);
    await this.saveCfg();
  },
  async openSettings() {
    this.section = "general";
  },
  async switchAsr(ev: Event) {
    if (!this.cfg) return;
    this.cfg.active_asr_id = (ev.target as HTMLSelectElement).value;
    await this.saveCfg();
  },
  async togglePolish() {
    if (!this.cfg) return;
    this.cfg.active_llm_id = this.cfg.active_llm_id ? "" : (this.cfg.llm_profiles[0]?.id ?? "");
    await this.saveCfg();
  },
  async switchMic(ev: Event) {
    if (!this.cfg) return;
    this.cfg.mic_device_uid = (ev.target as HTMLSelectElement).value;
    await this.saveCfg();
    try { this.activeMic = await invoke<[string, string] | null>("get_active_mic"); } catch {}
  },
  // 顶栏状态条: 当前组合一眼可见
  statusLine(): string {
    if (!this.cfg) return "加载中…";
    const asr = this.cfg.asr_profiles.find((p: any) => p.id === this.cfg.active_asr_id)?.name ?? "未设置";
    const llm = this.cfg.active_llm_id
      ? (this.cfg.llm_profiles.find((p: any) => p.id === this.cfg.active_llm_id)?.name ?? "关")
      : "关";
    const mic = this.activeMicName();
    const hk = this.hotkeyLabel() || "F5";
    return `${asr} · ${mic} · 按住 ${hk}`;
  },
  activeMicName(): string {
    return this.activeMic?.name ?? "自动(默认麦克风)";
  },
  // 详情面板: 把该条文本放进词典快加框(切到设置→词典页)
  addTermFromHist(h: any) {
    this.quickTerm = (h.final_text || h.raw || "").slice(0, 40);
    this.section = "dict";
  },
  filteredHist() {
    const q = this.histSearch.trim().toLowerCase();
    if (!q) return this.history;
    return this.history.filter((h: any) =>
      (h.raw || "").toLowerCase().includes(q) || (h.final_text || "").toLowerCase().includes(q));
  },
  async addQuickTerm() {
    const t = this.quickTerm.trim();
    if (!t) return;
    if (!this.cfg) return;
    if (this.cfg.dict.some((d: any) => d.term === t)) { this.quickTerm = ""; return; }
    this.cfg.dict.push({ term: t, variants: [], guard_words: [], boost: 3 });
    this.quickTerm = "";
    await this.saveCfg();
    await this.refreshHotwords();
  },
  filteredSections() {
    const q = this.search.trim().toLowerCase();
    if (!q) return this.sections;
    return this.sections.filter((s: any) => s.title.includes(q) || s.keywords.includes(q));
  },
});

// 页面片段注入(B47 拆分: index 骨架 + src/pages/*.html)
const pageHtml: Record<string, string> = { asr: pageAsr, history: pageHistory, general: pageGeneral, dict: pageDict, llm: pageLlm, about: pageAbout };
for (const [name, html] of Object.entries(pageHtml)) {
  const slot = document.querySelector(`[data-page="${name}"]`);
  if (slot) { slot.innerHTML = html; slot.removeAttribute("data-page"); }
}
Alpine.start();


