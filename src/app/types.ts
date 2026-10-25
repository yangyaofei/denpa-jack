// 主窗组件的公共类型(阶段A 第5步从 main.ts 拆出):
// 数据类型与 Rust settings.rs 对齐; AppThis 是组合式组件的完整形状——
// 每个页域模块导出 partial 对象, 方法签名以 AppThis 为 this 类型, 跨域调用经此类型检查。
// 与 Rust settings.rs 对应的类型(字段以该文件为唯一事实来源)
export interface LlmProfile {
  id: string; name: string; provider: string; base_url: string; model: string;
  api_key: string; prompt: string; thinking: boolean; effort: string;
}
export interface AsrProfile {
  id: string; name: string; provider: string; api_key: string; base_url: string; hotwords_enabled: boolean;
}
export interface DictEntry {
  term: string; variants: string[]; guard_words: string[]; boost: number;
}
export interface RegexRule { pattern: string; replacement: string; }
export interface HotkeyConfig {
  key: string; ctrl: boolean; alt: boolean; cmd: boolean; shift: boolean;
}
/// C55: 绑定集——一个动作可绑多个键, 单一真源(旧 hotkey/hotkeys 已停止写出)
export interface BindingSet { current: string[]; }
export interface Config {
  keys: string[]; llm_profiles: LlmProfile[]; asr_profiles: AsrProfile[];
  dict: DictEntry[]; normalizations: RegexRule[];
  active_llm_id: string; active_asr_id: string;
  bindings: Record<string, BindingSet>;
  use_llm_correction: boolean; clipboard_only: boolean;
  detect_active_target: string;
  auto_check_update: boolean;
  llm_timeout_secs: number; llm_max_tokens: number; llm_retries: number;
  activation: string; audio_feedback: boolean; restore_clipboard: boolean;
  keep_in_clipboard: boolean; trailing_newlines: number; ax_paste_only_apps: string[]; local_autostart: boolean; local_service_dir: string;
  builtin_engine: { model: string; vad: string; twopass: boolean; chunk_size_sec: number; max_context_sec: number; endpointing: string; max_sessions: number };
  overlay_position: string; history_limit: number;
  max_recording_seconds: number; min_recording_seconds: number; keep_audio_count: number;
  extra_tail_ms: number; mic_device_uid: string; mic_priority: string[];
  data_dir: string;
  /// 旧字段: 仅兼容旧配置文件读取, 后端 skip_serializing 不再返回
  hotkey?: HotkeyConfig; hotkeys?: HotkeyConfig[];
}
export interface HistoryRecord {
  ts: string; engine: string; raw: string; final_text: string;
  llm_used: boolean; delivered: string; audio_path: string; warning?: string;
}
export interface LocalSvcState {
  state: string; port: number; elapsed_secs: number | null; exit: string | null;
  tail: string[]; busy: boolean;
}

/// 组件完整形状: 状态字段 + 全部方法。各页域 partial 的 this 都是它。
export interface AppThis {
  // —— 公共状态(common) ——
  section: string; search: string; recording: boolean; partial: string; result: string;
  error: string; finalInfo: any; cfg: Config | null;
  /** 保存前快照的 data_dir(D10 提示用) */
  _prevDataDir: string;
  permWarning: string; perms: any[]; version: string;
  settingsOpen: boolean; prioSel: number;
  autostartOn: boolean; // 未在数据里声明的动态字段(toggleAutostart 用, 保留现状)
  mics: { uid: string; name: string }[]; activeMic: [string, string] | null; micToAdd: string;
  sections: { id: string; title: string; keywords: string }[];
  // —— 热键录制(general) ——
  hotkeyHint: string; hkCapturing: boolean; hkCurrent: string; hkFinal: string; _capTimer: any;
  // —— ASR 页 ——
  asrEdit: AsrProfile | null; asrIsNew: boolean; builtinDirty: boolean;
  localSvc: LocalSvcState;
  // —— LLM 页 ——
  llmEdit: LlmProfile | null; llmIsNew: boolean; llmModels: string[]; llmModelsError: string;
  selftesting: boolean; selftestResult: string; editLlmPromptPreview: boolean; defaultPrompt: string;
  // —— 词典页 ——
  dictEdit: any; dictIsNew: boolean; dictSearch: string; hotwordsPreview: string[]; quickTerm: string;
  // —— 历史页 ——
  history: HistoryRecord[]; selHist: HistoryRecord | null; histSearch: string; clearArm: boolean;
  // —— 通用页 ——
  axPasteOnlyText: string;
  // —— 关于页 ——
  updateState: "idle" | "checking" | "latest" | "available" | "error";
  updateCheckedAt: string; updateInfo: any; updateBusy: boolean; updateProgress: string; updateErr: string;

  // —— 方法(按域分组; 签名与实现一致, tsc 校验) ——
  // common
  init(): Promise<void>;
  initMock(): Promise<void>;
  loadCfg(): Promise<void>;
  saveCfg(): Promise<void>;
  toggleRecording(): Promise<void>;
  switchSection(id: string): void;
  openSettings(): Promise<void>;
  statusLine(): string;
  filteredSections(): { id: string; title: string; keywords: string }[];
  mdRender(src: string): string;
  uiSelftest(): Promise<void>;
  openConfigFile(): Promise<void>;
  toggleAutostart(): Promise<void>;
  openDataDir(): Promise<void>;
  micName(uid: string): string;
  refreshMics(): Promise<void>;
  activeMicName(): string;
  // 权限(common)
  missingPerms(): any[];
  permStatusText(p: any): string;
  checkPerms(): Promise<void>;
  requestMic(): Promise<void>;
  requestScreenPerm(): Promise<void>;
  openPermSettings(p: any): Promise<void>;
  // general
  describeCombo(s: string): string;
  hotkeyLabel(): string;
  hkList(): string[];
  removeHotkey(combo: string): Promise<void>;
  startHotkeyRecord(): Promise<void>;
  confirmHotkey(): Promise<void>;
  cancelHotkey(): Promise<void>;
  addFnPreset(preset: string): Promise<void>;
  setActivation(ev: Event): Promise<void>;
  setMicDevice(uid: string): Promise<void>;
  addToPriority(): Promise<void>;
  movePriority(i: number, dir: -1 | 1): Promise<void>;
  removePriority(i: number): Promise<void>;
  onDetectTargetChange(): Promise<void>;
  openScreenContextDir(): Promise<void>;
  switchMic(ev: Event): Promise<void>;
  // asr
  newAsr(): void;
  editAsr(p: AsrProfile): void;
  delAsr(p: AsrProfile): Promise<void>;
  useAsr(p: AsrProfile | { id: string }): Promise<void>;
  markBuiltinDirty(): void;
  restartBuiltin(): Promise<void>;
  pollLocalSvc(): Promise<void>;
  localSvcAction(act: "start" | "stop"): Promise<void>;
  localSvcText: string; // getter
  saveAsr(): Promise<void>;
  switchAsr(ev: Event): Promise<void>;
  // llm
  newLlm(): void;
  editLlm(p: LlmProfile): Promise<void>;
  delLlm(p: LlmProfile): Promise<void>;
  useLlm(p: LlmProfile): Promise<void>;
  saveLlm(): Promise<void>;
  fetchLlmModels(): Promise<void>;
  effortOptions(): string[];
  onLlmProviderChange(): void;
  togglePromptPreview(): Promise<void>;
  runLlmSelftest(): Promise<void>;
  togglePolish(): Promise<void>;
  // dict
  stars(boost: number): string;
  weightStars(b: number): string;
  filteredDict(): DictEntry[];
  newDict(): void;
  editDict(e: DictEntry): void;
  delDict(e: DictEntry): Promise<void>;
  saveDict(): Promise<void>;
  refreshHotwords(): Promise<void>;
  addTermFromHist(h: any): void;
  addQuickTerm(): Promise<void>;
  // history
  refreshHistory(): Promise<void>;
  copyHist(h: HistoryRecord): Promise<void>;
  openAudio(h: HistoryRecord): Promise<void>;
  playHist(h: HistoryRecord): Promise<void>;
  rerunHist(h: HistoryRecord): Promise<void>;
  clearHist(): Promise<void>;
  filteredHist(): HistoryRecord[];
}
