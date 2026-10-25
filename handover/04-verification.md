# 验证体系

原则：**没有真实执行的证据，就不算完成**。代码通过编译不代表行为正确；行为正确要有可复核的证据。

## 四层验证

| 层 | 工具 | 覆盖什么 | 通过标准 |
|---|---|---|---|
| 静态 | `cargo check --all-targets` | 类型、借用、警告 | 0 警告 0 错误 |
| 单元 | `cargo test` | 纯函数与规则（目标解析、窗口命中、文本处理、协议解析…） | 全部通过 |
| 前端 | `npm run build` + `node tests/frontend-checks.mjs` | 前后端字段一致、版本号六处一致、浮窗规则、队列规则 | 全部 PASS |
| 端到端 | 探针 + 自测通道 + 真机操作 | 真实系统 API、真实录音、真实交付 | 日志/产物可核对 |

## 探针（examples/）

只读诊断工具，不修改任何状态：

- `cargo run --example ctx_capture -- window|display`：截图路径、产物尺寸、目标描述。
- `cargo run --example mic_ids`：麦克风设备枚举（uid/名称/默认格式）。

探针输出是排障的第一手证据；排查系统 API 问题（AX、屏幕、音频）时必须先跑探针。

## 自测通道（VOICEMAC_AUTOTEST）

| 值 | 作用 |
|---|---|
| `e2e` | 真实麦克风录音 2.5s，走完整链路（ASR → 纠错 → 交付 → 历史） |
| `file` + `VOICEMAC_AUTOTEST_FILE=<wav>` | 用真实音频文件回放，替代麦克风输入 |
| `file` + `VOICEMAC_AUTOTEST_FILES=a.wav,b.wav,c.wav` | 多段回放（段间隔 400ms），验证分段队列 |
| `ui` | 页面数据链验证（函数结果 vs 磁盘真值） |
| `update` | 更新检查；`VOICEMAC_AUTOTEST_UPDATE=install` 连安装一起 |
| `quit` | 2 秒后走正常退出路径 |

注意：自测模式会跳过权限弹窗与单实例插件；新实例如果使用内置本地引擎，模型预热约 40s，文件通道在 1.2s 就开始，会报"内置引擎还在启动"——这是环境限制，不是缺陷。

## 日志证据（如何判读）

日志位置：`<数据目录>/app.log`。关键行：

- 截图：`[ctx] 截图已采集 path=… bytes=… ms=… sha256=… target=window(id 标题)` 或 `target=display(n id=… origin=… size=…)`
- 目标探测：`[ctx] AX 焦点不可用，回退全系统最前窗口`；`[ctx] 光标下没有可截取窗口，回退为屏幕截图`
- 浮窗：`[hud] 定位: 目标屏=Some(id)` / `目标屏=None`
- 交付：`[deliver] …`（AX 直写/粘贴/焦点变化）
- LLM：`[llm] …`（带图指纹、耗时、重试）

判读方法：一次录音一次链路；同一段日志里截图目标的 display 与浮窗的"目标屏"应当是同一个 id。

## 发布产物验证（不可省略）

```bash
gh release download vX.Y.Z -p "Denpa.Jack_X.Y.Z_aarch64.app.tar.gz"
tar xzf … && codesign --verify --strict "Denpa Jack.app"
/usr/libexec/PlistBuddy -c "Print :CFBundleShortVersionString" "Denpa Jack.app/Contents/Info.plist"
# latest.json 的 version 与 signature 长度（应为 428 字符左右）
```

## 验证失败时

- 先判断是环境问题还是代码问题（探针隔离系统 API）。
- 代码问题：修，不绕过；回归要补单测。
- 无法复现：如实说明，保留现场（日志/产物/截图），不假装通过。
