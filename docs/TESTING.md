# 测试组织（对齐 Handy 模式）

## 三层结构
| 层 | 命令 | 覆盖 | 状态 |
|---|---|---|---|
| 1. Rust 单元测试 | `cd src-tauri && cargo test --lib` | 纯逻辑+tempfile+重采样纯函数(audio resample_to_16k 提取), 全部不依赖 app 环境 | 70 用例 |
| 2. 前端冒烟 | 启动 dev server 后 `node tests/smoke.mjs <port>` | dev server 可用+关键 DOM(同 Handy Playwright 冒烟语义, 零依赖实现) | 4 检查 |
| 3. 链路回归+打包 | `bash scripts/regression.sh` | 真实人声文件回放→转写→交付→历史断言 + e2e 真麦 + 打包签名 | 每次打包必跑 |

## 修 bug 的测试规则（TEST-MATRIX.md 登记）
每修一个 bug 必加对应用例（Handy settings.rs 内联 JSON fixture / history.rs 内存库相同做法）。

## Handy 对照（调研结论, subagent 实测数据）
- Handy: 29 文件 ~270 单元测试(纯函数+tempfile+内存 SQLite+17 tokio::test 本地 socket)+Playwright 冒烟 2 例+CI test.yml；**无集成测试目录**(tests/ 是 playwright 的)——与我们同构
- 我们差距补齐: tempfile 手法✅/前端冒烟✅/链路回归✅(regression.sh, Handy 无此层——我们的 autotest 通道更强)/tokio::test 层暂缺(当前异步逻辑少, 需要时补)
- 无 CI(本地单机项目), regression.sh 即 CI 等价物
