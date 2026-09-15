macOS（Apple Silicon）构建，用项目自签证书签名。

安装：下载 `.dmg`（或 `.zip`）→ 拖进「应用程序」。

首次打开若被 Gatekeeper 拦下（提示无法验证开发者）：
系统设置 → 隐私与安全性 → 「仍要打开」；
或执行 `xattr -dr com.apple.quarantine "/Applications/Denpa Jack.app"`。

权限：首次启动需要授予「麦克风」与「辅助功能」（app 内有权限卡片会提示并指引）。

校验和见同目录的 `SHA256SUMS_*.txt`。
