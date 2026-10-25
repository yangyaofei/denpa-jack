# Denpa Jack 需求与架构文档（SPEC）

> 依据：2026-10-24 阶段A 结构重构后的源码全量对齐（src-tauri/src/ 含 engine/ delivery/ config/ 子目录，
> src/app/ 按页拆分；全部行为描述经代码核验）。历史版本（0.3.29 基线）的逐文件行数已删，
> 模块地图以本文为准；跨边界契约见 `docs/CONTRACTS.md`；功能验收状态见 `docs/FUNCTION-CHECKLIST.md`。

## 1. 项目定位

- macOS 菜单栏语音输入工具：按住全局热键说话 → ASR 流式转写 → 本地词典纠错 → 可选 LLM 润色 → 自动写入当前光标位置。
- 个人工具，Tauri 2 实现：Rust 后端（src-tauri/）+ Alpine.js 轻前端（src/）。
- 两个窗口（UI v6 单窗化后）：
  - `main`：主窗（历史 + 六页设置，1060x680，最小 760x480），页面 index.html + `src/pages/*.html` 六片段（`?raw` 注入）。
  - `hud`：录音浮窗，480x200（队列态 664 宽），透明/置顶/无边框/默认隐藏，页面 hud.html。
- 常驻：`ActivationPolicy::Accessory`（菜单栏，不占 Dock）；关窗=隐藏；退出走托盘。
- 单实例：tauri-plugin-single-instance。
- 系统权限：麦克风（必需）、辅助功能（必需，AX 直写/粘贴/键盘监听都走它）、屏幕录制（可选，仅截图上下文开启时）。权限卡在关于页与通用页展示。

## 2. 需求清单（用户视角）

> 逐功能的预期/试法/验收状态见 `docs/FUNCTION-CHECKLIST.md`（收口期台账）。此处只写行为定义。

### 2.1 核心流程

- **按住热键 = 录音**：
  - 默认 F5；绑定集 `bindings.transcribe.current[]` 支持一动作多键（C54/C55）。
  - 按下后 hud 浮窗出现在活动显示器，位置按 `overlay_position`（底部/中间/顶部）。
  - partial 文本流式显示（豆包/local 全量替换语义）；底部音量条随 RMS 跳动（120ms 采样，0-1000 定点）。
  - 可选提示音：开始 Tink / 结束 Pop / 错误 Basso。
- **松开 = 停止并结算**：
  - 尾音缓冲 `extra_tail_ms`（默认 0，上限 2000ms）。
  - 结算链：ASR Finish → 恰好一次 Result/NoSpeech/Error → **分段队列串行** → 词典纠错 →（可选）LLM → 交付 → 入历史。
  - 浮窗显示阶段（"… 转写中"/"✦ AI 润色中…"/"交付中…"），交付完成才收起。
- **Esc = 丢弃**：录音期间起键盘全键观察（`shortcut::esc_listen_begin`，任意修饰键状态识别 Esc）→ 协调器 Cancel → Abort；在途结果只入历史不粘贴。**已知问题 R2：真机仍无效，收口期排查**。
- **转写中可开新录音**：协调器允许转写期间再次按下直接 Start；各段经 `segment_queue` FIFO 串行交付（顺序恒等于录音顺序，粘贴事务不互相踩）。
- **引擎三选一**（ASR 档案 provider，D9 后 OpenAI Realtime 已移除）：
  - 豆包流式（volcengine，主力）。
  - 智谱 GLM-ASR（zhipu，文件式，无 partial）。空转写归 NoSpeech 金标（D2）。
  - 本地网关（local）：`engine/gateway.rs` 管理的 denpa-asr 子进程（mlx-qwen3-asr），随机端口登记、零配置（BUILTIN_ID="builtin" 特化档案）；WS 走 OpenAI Realtime 事件面子集；单模型单流。
- **交付链**（pipeline::deliver，前一步成功即止；D3 后原剪贴板快照在交付入口取）：
  1. 前台是本 app → 仅复制（`copied-self`）。
  2. AX 直写（`AXSelectedText` + 多信号回读校验；`clipboard_only=false` 且未命中粘贴优先名单时尝试；成功=`pasted-ax`）。
  3. `clipboard_only=true` → 仅复制（`copied`）。
  4. 焦点已变 → 仅复制（`focus-changed-copied-only`）。
  5. 回执式可靠粘贴（`delivery/paste.rs`，`pasted-cmdv`）：剪贴板延迟供数据（lazy promise）→ 目标 app 读取=回执 → CmdV chord 注入 → 末次回执后 200ms 安静期恢复**交付前的原剪贴板**（D3）；changeCount/ownership 守卫绝不覆盖用户新复制；恢复硬上限 8s；`restore_clipboard=false` 保留转写文本。
  - `trailing_newlines`（0-10）只改交付文本末尾换行，不发回车键。

### 2.2 主窗（index.html + src/app/ 按页组件）

- 顶栏：ASR 引擎下拉 / 润色开关 / 麦克风下拉 / 试音按钮（与热键录音状态经 core 轮询互相同步）/ 状态条（引擎·麦克风·热键）。
- 六页（`sections`，hash 路由 + 搜索过滤）：历史 / 词典 / 通用 / 语音识别 / 大模型 / 关于。
- 前端组件按页拆分（`src/app/`）：`types.ts`（AppThis 完整形状）+ `util.ts` + `common.ts`（横切：轮询/配置/权限/自测）+ 六页域文件（asr/llm/dict/history/general/about），`main.ts` 组合（44 行）。
- 同步模型：**事件 + 轮询兜底**（`docs/SYNC.md`）；主窗 800ms 轮询 `poll_versions` 四计数（history/config/mics/recording），值变化才刷新对应数据；hud 150ms 轮询 `hud_poll`。

### 2.3 各页功能

- **历史**：双栏列表/详情；搜索（原文+纠错稿）；复制/播放/显示录音/重转（**重转语义 R1 待重做**：应为后台重算回写条目，现状走语音重放）/加为词条。
- **词典**：快加行（回车入库 boost=3）/搜索/表格（词条·变体·护栏词·权重★☆）/编辑。
- **通用**：热键录制器（C55 后端录制，Esc 取消）/F 预设/多键绑定列表/激活方式（hold|press）/活动目标探测三档/粘贴优先名单/数据目录（改后提示重启，D10）/自启动/打开配置与数据目录。
- **语音识别**：Key 池/引擎档案卡（volcengine|zhipu|local）/热词预览（预算后，改词典即时生效）/内置网关卡片（启停/重启生效/启动走秒/崩溃退出码/日志尾部）/当前设备/麦克风优先级。
- **大模型**：档案卡/编辑（供应商切换自动填地址+拉模型列表/思考开关/思考强度/提示词+Markdown 预览/自测走真实链路，`llm::selftest`）/调用参数（超时/最大 token/重试）。
- **关于**：版本（`app_version` 命令）/检查更新/安装更新/权限三项。

### 2.4 失败处理

- **ASR 失败（Error）**：失败段入队（先落盘+写历史保证可重试），浮窗录完提示一次；队列重试（`queue_retry`）而非旧 retry_last（已删）。
- **空转写（NoSpeech）**：金标语义（D2）——写一条空历史 + 浮窗提示 2.5s，不进失败段、不报警。
- **LLM 失败/关闭**：交付**词典纠错版**（D1）；永不阻塞交付；warning 记入历史。
- **太短丢弃**：`min_recording_seconds`（默认 0.3s，下限 0.05s）生效（D5）→ Abort 引擎、不入管线。
- **超长截断**：`max_recording_seconds`（默认 1800）到时 B5 定时器发 `AutoStopDue` 信号（决策统一协调器，哑火根因已修），自动收尾可立即再录。
- **麦克风 open 失败**：清缓存重试一次；权限拒绝记日志（`[audio]` 前缀）。
- **配置读取失败**：该段按 Failed 处理（warning=配置读取失败）。
- **管线 panic**：`catch_unwind` 兜底，HUD 不卡。

## 3. 架构（src-tauri/src/，2026-10-24 阶段A 后）

### 3.0 数据流总纲

热键/托盘/试音 → `send_ctrl`（单一真源）→ **协调器**（三规则状态机）→ 效果线程执行 `ctrl_start/ctrl_stop/ctrl_abort` → 采集（audio.rs，cpal 16k mono i16le 200ms 分片）+ 引擎会话（engine/，专用 current_thread runtime）→ AsrEvent → **分段队列**（segment_queue，单 worker 串行）→ `pipeline::post_process`（词典 → 可选 LLM → 交付 → 历史）→ delivery/（AX 直写 | 回执式粘贴）→ core 版本计数 bump → 前端轮询刷新。

- `core.rs`：全局可变状态唯一居所（CoreState：队列/会话/配置·历史·设备三计数，一把 std Mutex + 版本号）；前端轮询协议的读接口。
- `transcription_coordinator.rs`：纯状态机（无副作用）；Effect 由专职效果线程执行。
- `recording.rs`：会话状态机（RecordingSession 闭包；resolve/编排）。麦克风解析在 `audio.rs`，截图编排在 `screen_context.rs`。
- `engine/`：`contract.rs`（Cmd/AsrEvent）+ `mod.rs`（trait AsrEngine + spawn_session 分派 + resolve_asr + settle）+ `doubao.rs`/`zhipu.rs`/`local.rs` 三引擎 + `gateway.rs`（网关子进程生命周期：随机端口登记/探活收尸/两段式停止/日志尾部）。
- `segment_queue.rs`：FIFO 队列 + 单 worker（100ms 轮询驱动）；成功/失败/NoSpeech 三类落历史与音频。
- `pipeline.rs`：SessionHandoff（pcm/focus/gen/audio_path/screenshot）+ post_process + deliver 编排 + 时间戳。
- `delivery/`：`ax.rs`（AX FFI/AxGuard RAII/ax_insert/ax_trusted）+ `mod.rs`（frontmost_bundle/FocusLock/cg_key_tap）+ `paste.rs`（回执式粘贴 + ClipboardSnapshot）。
- `dict.rs`：三级纠错管道（正则规范化 → 变体精确替换 → 拼音窗口模糊）+ 热词预算（budget_hotwords，100 token 分层）。
- `llm.rs`：OpenAI 兼容调用（submit_corrected_text 工具优先/content 兜底）+ DEFAULT_LLM_PROMPT + selftest + list_models + tool_used 直读。
- `history.rs`：HistoryRecord JSONL + save_wav + wav_header/wav_data_chunk（FLLR 安全）+ prune。
- `config.rs` + `config/paths.rs`（APP_ID/data_dir 恒定解析/write_config_atomic）+ `config/migrate.rs`（一次性迁移：bindings/F5 兜底/内置引擎/VAD 归零）。
- `active_target.rs` + `screen_context.rs`：活动目标一次探测（焦点窗口→指针窗口→指针屏），截图与浮窗共用；ScreenCaptureKit 进程内截图（弃 screencapture 命令行——曾卡 134.6s）。
- `hud.rs`/`overlay.rs`：浮窗内容接口（唯一写者）/坐标与窗口（build.rs 契约闸：坐标 API 只准 overlay.rs）。
- `tray.rs`/`tray_events.rs`：objc2 NSStatusItem；状态行三点刷新（ctrl_start 尾/stop/abort）。
- `shortcut.rs`：handy-keys HotkeyManager（transcribe 绑定）+ C55 后端组合录制 + Esc 全键观察。
- `permissions.rs`/`mic_watch.rs`/`log.rs`/`crash_log.rs`/`update.rs`/`autotest.rs`：权限/设备热插拔监控/日志/崩溃捕获/应用内更新/自测通道。
- `commands.rs`：Tauri command 薄壳（业务在域内；llm_selftest 已下沉 llm::selftest）。

### 3.1 关键机制（保留有效的细节描述）

- **会话闭包**：RecordingSession drop = cancel 置位 = 附属线程作废；无会话级 static（CONTRACTS 生命周期节，build.rs 不检查此条——评审保证）。
- **会话代令牌**：SESSION_GEN/CANCELLED_GEN（pipeline.rs）防取消段被交付——已知单槽缺陷（连续取消两段时前一段可能漏判，审核 a-3，收口期候选）。
- **引擎协议**：豆包二进制帧（gzip 全量请求/音频/收尾帧，自校正解析，X-Api-Key 连接级鉴权，热词 corpus 直传）；智谱 multipart 文件式；local=WS Realtime 子集（首帧 context 注入热词）。恰好一次 Result|NoSpeech|Error。
- **B5 超长**：定时器只发信号（AutoStopDue），决策在协调器（f9032c5 修复哑火）。
- **事件面（真实）**：前端 listen 只有 `config-changed`（托盘切换配置时发）与 `update-progress`（下载进度）。Rust→WebView 的其余广播通道在打包版不可达（C43b 实测），一切状态同步走 invoke 轮询（core 版本计数 / hud_poll）。引擎事件桥的 "asr-partial"/"asr-result" 等字符串是**内部分派标签**，不是前端事件。
- **AppState**（lib.rs）：session（当前会话）/last_audio/engine_mirror——收口期候选：并入 core。

### 3.2 契约闸与自测

- build.rs 递归扫描 src/：坐标 API 只准 overlay.rs（mouseLocation 豁免 autotest.rs）；主线程 API 必须带调度标记；hud 快照 API 白名单。违反=编译 panic。
- autotest.rs：`VOICEMAC_AUTOTEST=file`（文件回放全链）`timelimit`（超长截断+哑火 4 断言）`ui`（主窗数据链 vs 磁盘真值；**需先起 vite dev server**，无 dev server 时 WebView 空白=假绿）`e2e`（真麦）`update[/install]`。`VOICEMAC_DATA_DIR` 隔离数据目录。
- 日志：`<数据目录>/app.log`，前缀 `[diag]/[voice]/[ui]/[audio]/[perm]/[shortcut]/[queue]/[pipeline]/[paste-tx]/[llm]/[ctx]/[hud]/[deliver]`。

## 4. 数据与配置

- **config.json**：固定在 `~/Library/Application Support/io.github.yangyaofei.denpajack/`（锚点）。全部字段 `#[serde(default)]`；原子写（write_config_atomic：tmp+rename）。
  - `data_dir`：数据根目录（history/recordings/llm_logs/app.log 跟随）。**启动解析一次恒定，改后重启生效（D10）**。取路径统一 `config::data_dir(app)`。
  - `keys[]`（Key 池）、`llm_profiles`（provider=deepseek|zhipu）、`asr_profiles`（provider=volcengine|zhipu|local）、`active_llm_id`/`active_asr_id`。
  - `dict[]`（term/variants/guard_words/boost）、`normalizations[]`。
  - `bindings`（C54 绑定集，旧 hotkey/hotkeys 只读迁移）；`activation`（hold|press）。
  - 开关与数值：`use_llm_correction`/`clipboard_only`/`audio_feedback`/`restore_clipboard`/`trailing_newlines`/`detect_active_target`（off|active_window|active_display）/`llm_timeout_secs`(30)/`llm_max_tokens`(0)/`llm_retries`(2)/`auto_check_update`/`overlay_position`/`history_limit`(200)/`keep_audio_count`(50)/`max_recording_seconds`(1800)/`min_recording_seconds`(0.3)/`extra_tail_ms`/`mic_device_uid`/`mic_priority`/`local_service_dir`/`builtin_engine{}`。
- **history.jsonl**：一行一条；含 NoSpeech 空记录与失败段记录（可重试）。
- **recordings/**：`rec_{millis}.wav`（16k mono i16le，44B 头）；按 keep_audio_count 裁剪。
- **llm_logs/**：每次调用全量 JSON（url/request/response_raw；截图换占位；key 不写）。
- **screen_context/**：截图归档，保留 200 张。

## 5. 构建与签名

- 开发：`npm run tauri dev`（vite :1420）。打包：`npm run tauri build`（app+dmg；updater 产物私钥缺失报错不阻塞 app 产出，发版时 CI 注入）。
- 签名：rcodesign 读 certs/*.p12 覆盖签名；identifier `io.github.yangyaofei.denpajack` 恒定 → TCC 持久。详见 `docs/CODE-SIGNING.md`。
- 版本号六处一致（package.json/package-lock×2/Cargo.toml/Cargo.lock/tauri.conf.json，frontend-checks 断言）；tag `v*` 触发 release.yml 构建签名发 Release。
- 应用内更新（update.rs + tauri-plugin-updater）：minisign 强制验签；自下载不带 quarantine → TCC 不变；`auto_check_update` 只报告不自动装。
- Rust 关键依赖：tauri 2（macos-private-api）、cpal、tokio-tungstenite、reqwest、objc2 系、handy-keys（fork 固定 rev，含两个 macOS 键盘状态修复）、pinyin、flate2。

## 6. 已知边界 / 遗留（2026-10-24 收口期清单）

- **R1 重转语义重做**（REQUIREMENTS.md 已钉死）：后台重算+回写条目+三态显示，不交付不弹 HUD。
- **R2 Esc 取消真机无效**：已修一版（单键注册→全键观察），仍无效；候选=覆盖安装未重启旧进程/监听器并存问题；日志锚点 `[shortcut] Esc 取消命中`。
- 热词 100 token 预算仅豆包/local 注入；智谱不支持。
- CANCELLED_GEN 单槽：连续取消两段时前一段在途结果可能漏判（审核 a-3）。
- `now_local_pub` 为 UTC+8 近似格式化（伪秒 :60 修过跨秒回退）；非本地时区用户时间戳偏差——候选改系统本地时间。
- AppState（session/last_audio/engine_mirror）未并入 core——收口期候选。
- 隐私：release 转写内容不落日志；llm_logs 明文（本地磁盘）。
- 死代码已清（56b8c9e）；AGENTS.md 为模块结构权威；handover/ 为流程文档。
