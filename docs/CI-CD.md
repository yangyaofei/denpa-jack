# CI/CD、签名与发版运维（Denpa Jack）

> 外部运维唯一文档：只记录无法写在代码注释里的内容（密钥与签名身份、TCC 权限与分发边界、GitHub Secrets、CI/CD 流水线、发版与排障）。架构、跨边界契约、同步机制、依赖补丁与崩溃诊断已全部收口进对应源码顶部注释与 [AGENTS.md](../AGENTS.md)。

## 1. 密钥与签名身份（本地 `certs/`，永不入库）

签名使用 [rcodesign](https://github.com/indygreg/apple-platform-rs) 直接读取 `.p12` 文件（不导入钥匙串、不要求系统信任——Apple `codesign` 对未信任自签证书会直接报 `no identity found`）。

- **固定身份（绝不轻易更换）**：Bundle ID `io.github.yangyaofei.denpajack` + 10 年自签证书 `Denpa Jack Dev`。macOS TCC 按「指定要求 DR = identifier + 证书叶指纹」记录权限，换任意一项都会重置全机麦克风/辅助功能授权。
- **两套独立凭据**：代码签名 `.p12` 管运行与 TCC 延续；minisign 密钥管 `tauri-plugin-updater` 更新包验签（强制开启，无法关闭）。两者都需妥善备份。

| 资产 | 本地位置（gitignored） | 用途 | GitHub Secret |
|---|---|---|---|
| 代码签名证书（10 年 p12） | `certs/denpa-jack-dev-10y.p12` | `rcodesign sign` 签 `.app` | `APPLE_CERT_P12_BASE64`（base64） |
| p12 密码 | `certs/denpa-jack-dev-10y.pw` | 解 p12 | `APPLE_CERT_PASSWORD` |
| minisign 更新私钥 | `certs/denpa-jack-updater.key` | 签发 `.app.tar.gz.sig` / `latest.json` | `TAURI_SIGNING_PRIVATE_KEY` |
| minisign 私钥口令 | `certs/updater-signing.pw` | 解 minisign 私钥 | `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |
| Homebrew Tap PAT（可选） | — | 发版后自动回写 `yangyaofei/homebrew-tap` 的 `Casks/denpa-jack.rb`（未配置时提示并跳过，不阻断发版） | `HOMEBREW_TAP_TOKEN` |

> 四项必须存为 GitHub **Secrets**（绝不放明文 Variables，公开仓库 Variables 人人可见）。

重建自签证书（仅丢失时，**会更换身份并触发重新授权**）：

```bash
openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem -days 3650 -nodes \
  -subj "/CN=Denpa Jack Dev" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning"
openssl pkcs12 -export -legacy -out certs/denpa-jack-dev-10y.p12 -inkey key.pem -in cert.pem -passout pass:"<密码>"
```

## 2. CI / Release 流水线（`.github/workflows/`）

| 工作流 | 触发 | 作业与检查 |
|---|---|---|
| `ci.yml` | push `master` / PR / 手动 | `macos-15`：tracked-file 守卫（禁密钥/`tmp/`/真人录音）→ `npm ci` → `npx tsc --noEmit` → `npm run build` → `node tests/frontend-checks.mjs` → `cargo test --lib` → `cargo check`（含 `build.rs` 契约闸） |
| `release.yml` | push `v*` tag / 手动 `workflow_dispatch` | `macos-15`：注入 tag 版本号 → `npm run tauri build -- --bundles app` → `rcodesign` 签名 + `Authority=Denpa Jack Dev` 断言 → 签后重打更新包（`scripts/package-updater.sh`）+ 生成校验 `latest.json` → 打 `.dmg`/`.zip`/`SHA256SUMS` → 发布 GitHub Release |

> CI 不跑依赖本机麦克风/密钥的链路；本地发版前跑 `./scripts/regression.sh`。

## 3. 发版与本地演练

```bash
# 正式发版（推非 prerelease tag；/releases/latest/ 只认正式 Release）
git tag v0.4.0 && git push origin v0.4.0
```

本地打包与签名验证（需安装 `rcodesign`，见 [apple-platform-rs Releases](https://github.com/indygreg/apple-platform-rs/releases)）：

```bash
npm run tauri build -- --bundles app
~/.local/bin/rcodesign sign \
  --p12-file "certs/denpa-jack-dev-10y.p12" \
  --p12-password-file "certs/denpa-jack-dev-10y.pw" \
  "src-tauri/target/release/bundle/macos/Denpa Jack.app"
codesign --verify --deep --strict "src-tauri/target/release/bundle/macos/Denpa Jack.app"
```

Release 产物：`.dmg`（拖入应用程序）、`.app.zip`（`ditto` 保留签名与符号链接）、`.app.tar.gz` + `.sig`（应用内更新包，**必须在代码签名后由 `scripts/package-updater.sh` 生成**）、`latest.json`、`SHA256SUMS_<ver>.txt`。

## 4. 应用内更新、TCC 权限与跨机分发

- **三项系统权限**：麦克风（必需，录音）、辅助功能（必需，热键监听与 AX/Cmd+V 写入）、屏幕录制（可选，仅「截图上下文」开关开启时）；**不需要输入监控**。
- **热更新免重授权**：应用自主下载不带 `com.apple.quarantine`，且新包满足同一 `Denpa Jack Dev` 签名要求，升级后 TCC 权限无缝延续。
- **跨机首装**：自签证书未公证，浏览器/AirDrop 下载带隔离标记会被 Gatekeeper 拦截，需「系统设置 → 隐私与安全性 → 仍要打开」或执行 `xattr -dr com.apple.quarantine "/Applications/Denpa Jack.app"`。零摩擦分发需 Apple Developer ID + 公证。

## 5. 常见运维排障

| 现象 | 原因与处理 |
|---|---|
| `codesign: no identity found` / `CSSMERR_TP_NOT_TRUSTED` | 未信任自签证书被系统 `codesign` 拒用，统一用 `rcodesign` 签名 |
| 更新后 TCC 权限重置 | `tauri build` 默认产物早于 `rcodesign` 签名；务必由签名后的 `.app` 经 `scripts/package-updater.sh` 重打更新包 |
| 本地 `tauri build` 报缺少 `TAURI_SIGNING_PRIVATE_KEY` | 仅影响 `.app.tar.gz` 签名，`.app` 本体已产出；随后执行 `rcodesign sign` 即可（CI 自动注入私钥） |
