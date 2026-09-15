# 测试矩阵（cargo test --lib）
原则: **每次出问题, 修复必须附带回归测试**（bug → 用例映射见下）。
FFI/窗口/网络等无法单测的走 autotest 通道（VOICEMAC_AUTOTEST_FILE / e2e）。

## 覆盖表
| 模块 | 用例数 | 覆盖内容 |
|---|---|---|
| dict | 6 | 变体替换/护栏词/已含跳过/防子串自替换/正则规范化/最长优先 |
| transcription_coordinator | 8 | Idle+按→Start/录音+松→Stop/按住重复按忽略/无录音松开 noop/双 Stop 幂等/Cancel→Abort/Idle Cancel noop/全周期重启 |
| history | 2 | **C40** wav_header 逐字段 PCM 合法性 / HistoryRecord 往返 |
| doubao | 4 | 帧构造解析(无 seq)/带 seq 帧/垃圾输入 None/gzip 往返；C33/C30 标注走集成 |
| recording | 6 | **C40 双输入** wav_data_chunk 跳 FLLR 填充/垃圾拒绝 / **C8** 热词 100token 预算 / resolve_asr 未知引擎拒绝+首档回退+空 Key 取用公共 Key 池 |
| settings | 4 | normalize_key/shortcut_str/**C17 缺字段不清空**（顺带修出 serde 默认值 0 的隐患）/配置往返 |
| llm | 4 | tool_calls 提取/降级 content/空 None/畸形 arguments 降级 |

## bug → 用例 映射（修复必登记）
| 约束 | bug | 用例位置 |
|---|---|---|
| C40 | wav_header 半字节→audio_format=257 | history::wav_header_valid |
| C40b | 双输入混流(FLLR/ctrl_start 残留) | recording::wav_data_chunk_skips_fllr |
| C17 | 配置缺字段清空/默认 0 | settings::c17_deserialize_partial |
| C8 | 热词预算溢出 | recording::budget_hotwords |
| C33/C20/C30/C34/C35/C37 | 引擎语义(全量替换/不中途结算/uid 唯一/收尾超时/runtime 隔离/单通道) | 集成: autotest 文件回放(34.8s 多句音频无重复+松开结算) |
| C13/C26 | 主线程死锁 | 由 build.rs 契约闸+代码评审, 单测不可达 |
