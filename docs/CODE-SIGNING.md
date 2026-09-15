# 代码签名与权限（Code Signing & Permissions）

本文记录本项目的签名方案、为什么这么选、以及遇到的问题。**结论先行**：签名用
[rcodesign](https://github.com/indygreg/apple-platform-rs)（apple-codesign）直接读 `.p12`
文件完成，**不需要把证书导入钥匙串，也不需要系统信任它**。

## 1. 为什么是自签 + rcodesign

| 需求 | 自签 + rcodesign | 说明 |
|---|---|---|
| 本机长期不会丢失权限 | ✅ | TCC 按「指定要求 (DR)」记录：`identifier + certificate leaf`，只要证书和 bundle id 不变，重打包不重新授权 |
| 证书只是文件夹里的文件 | ✅ | `.p12` + 密码文件，签名命令直接读；不导入钥匙串、不进系统信任设置 |
| 将来能上 CI/CD | ✅ | rcodesign 是纯 Rust，可在 Linux/Windows runner 上签名（官方提供 GitHub Action） |
| 给别人的机器分发（零摩擦） | ❌ | 自签不满足 Gatekeeper（策略要求 `anchor apple generic`），别人下载需手动放行；要做到零摩擦只能 Developer ID + 公证 |

bundle id 固定为 `io.github.yangyaofei.denpajack`，证书 CN 为 `Denpa Jack Dev`（10 年）。
两者都是「代码要求」的组成部分，**改了就要重新授权一次**。

## 2. 实测证据

**A. `codesign` 用未信任的自签证书会失败**（这是硬性要求）

```
$ security find-identity "$KC"
  1) A807B258… "Denpa Jack Dev" (CSSMERR_TP_NOT_TRUSTED)

$ codesign --keychain "$KC" -s "Denpa Jack Dev" "Denpa Jack.app"
Denpa Jack Dev: no identity found
```

用 SHA-1 指定身份同样失败。Apple 论坛的解释（Quinn，thread 712043）：证书需要有到
「可信锚」的信任链，自签证书自己就是锚，必须显式 `security add-trusted-cert -r trustRoot`
设为信任根；Apple 签发的证书因为根/中间证书已在系统信任库里，才不需要这一步。

**B. rcodesign 不需要信任，产出的签名照样通过验签**

```
$ rcodesign sign --p12-file dev.p12 --p12-password-file dev.pw "Denpa Jack.app"
signing bundle at Denpa Jack.app into Denpa Jack.app
signing main executable Contents/MacOS/denpa-jack
creating cryptographic signature with certificate Denpa Jack Dev

$ codesign -dvvv "Denpa Jack.app" | grep -E "Identifier|Authority"
Identifier=io.github.yangyaofei.denpajack
Authority=Denpa Jack Dev

$ codesign --verify --deep --verbose=2 "Denpa Jack.app"
Denpa Jack.app: valid on disk
Denpa Jack.app: satisfies its Designated Requirement
```

**C. TCC 权限正常**（这是最需要实测的一点）

新 bundle id + 未信任证书 + rcodesign 签名，装上后跑 `VOICEMAC_AUTOTEST=e2e`：

```
[audio] 打开设备 name=Wireless Mic Rx … ch=2 fmt=F32
[audio] 会话采集: bytes=139128 chunks=21 peak=0.0284 按住时长=4326ms
[doubao] 帧 definite=true len=8 text=是个多个地儿呢。
```

麦克风授权后采集与转写均正常 → **自签 + rcodesign 能满足本机自用**。

### 需要的权限：三项里只有两项必需

| 权限 | 是否必需 | 用在哪 | 说明 |
|---|---|---|---|
| 麦克风 | ✅ | 录音 | 授权状态用 AVFoundation `authorizationStatusForMediaType` 查（权威，非"试着录一下猜"）；缺失时按下热键**直接拒绝并提示** |
| 辅助功能 (Accessibility) | ✅ | ①键盘监听与快捷键录制（handy-keys 的 `HotkeyManager`/`KeyboardListener`，库以 `AXIsProcessTrusted` 为准）②AX 直写光标 / 自动粘贴 | 缺它仍能录音，但结果只能进剪贴板（会明确提示） |
| 输入监控 (Input Monitoring) | ❌ 不需要 | —— | 只有"自研 CGEventTap listen-only 引擎"才需要它，该引擎已在 C54 删除；esc 用 Carbon 全局快捷键，不需要任何 TCC |

实机验证：本机只授予「辅助功能」（日志 `[deliver] ax_trusted=true`），热键、快捷键录制、自动粘贴全部正常；
启动时的权限总检查也只报告麦克风 / 辅助功能两项（见 `src-tauri/src/permissions.rs`）。

## 3. 资产与位置（仓库内 `certs/`，已 gitignore）

| 文件 | 位置 | 说明 |
|---|---|---|
| 证书+私钥 | `certs/denpa-jack-dev-10y.p12` | 10 年有效期，密码保护 |
| 密码 | `certs/denpa-jack-dev-10y.pw` | 供脚本 `--p12-password-file` 使用（chmod 600）；也可改为密码管理器 + 环境变量 |
| API 密钥 | `certs/secrets.env` | 本地开发用的 `VOLCENGINE_API_KEY` / `ZHIPU_API_KEY` / `DEEPSEEK_API_KEY`，不入库 |
| 生成产物 | 仓库内 `tmp/certs/`（gitignored） | `key.pem` / `cert.pem` / `dev.p12`；临时工作副本，可随时重生成 |
| 旧证书 | 已从钥匙串与信任设置中移除 | 旧身份 `VoiceInput Dev` 不再使用 |

`certs/` 整个目录在 `.gitignore` 里，不会被提交；CI 需要的两份材料（p12 的 base64 与密码）
放 repository secrets，见 [CI-CD.md](CI-CD.md)。

生成（如证书丢失需重建，注意：**重建=新身份=要重新授权一次**）：

```bash
openssl req -x509 -newkey rsa:2048 -keyout key.pem -out cert.pem -days 3650 -nodes \
  -subj "/CN=Denpa Jack Dev" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning"
openssl pkcs12 -export -legacy -out dev.p12 -inkey key.pem -in cert.pem -passout pass:"<密码>"
```

## 4. 打包与签名流程

```bash
npm run tauri build                     # 注意：signingIdentity 已从 tauri.conf.json 移除 →
                                        # tauri 会做 ad-hoc 签名（Identifier=denpa_jack-xxxx），随后被覆盖
~/.local/bin/rcodesign sign \
  --p12-file "certs/denpa-jack-dev-10y.p12" \
  --p12-password-file "certs/denpa-jack-dev-10y.pw" \
  "src-tauri/target/release/bundle/macos/Denpa Jack.app"
codesign --verify --deep "src-tauri/target/release/bundle/macos/Denpa Jack.app"
```

安装 / 启动：

```bash
rm -rf "/Applications/Denpa Jack.app"
cp -R "src-tauri/target/release/bundle/macos/Denpa Jack.app" /Applications/
open "/Applications/Denpa Jack.app"
```

`scripts/regression.sh` 的第 5 步已按上面写成 rcodesign 版本。

安装 rcodesign（官方预编译包，含校验文件；也可 `cargo install apple-codesign`）：

```bash
gh release download "apple-codesign/0.29.0" -R indygreg/apple-platform-rs \
  -p "apple-codesign-0.29.0-aarch64-apple-darwin.tar.gz*" --clobber
tar xzf apple-codesign-0.29.0-aarch64-apple-darwin.tar.gz
mkdir -p ~/.local/bin && cp apple-codesign-0.29.0-aarch64-apple-darwin/rcodesign ~/.local/bin/
```

## 5. 换机器 / 重装系统

```bash
security import certs/denpa-jack-dev-10y.p12 \
  -k ~/Library/Keychains/login.keychain-db -P "<密码>" -T /usr/bin/codesign -A
# 用 rcodesign 的话这一步只是为了备用（例如日后想用 codesign 手动验证）
```

- 只把 `.p12` + `.pw` 带过去即可；rcodesign 直接可用，**不需要任何信任设置**
- TCC 授权是「每台机器 per-user」的，新机器上首次使用要重新授权（与证书无关）

## 6. 在别人的 Mac 上运行

| 传输方式 | 结果 |
|---|---|
| U 盘 / `scp` / `rsync`（无隔离标记） | 直接能跑；权限需在对方机器授权 |
| AirDrop / 浏览器 / 邮件 / 解压 zip（带 `com.apple.quarantine`） | Gatekeeper 拦截，需 *系统设置 → 隐私与安全性 → 仍要打开*；或 `xattr -dr com.apple.quarantine "/Applications/Denpa Jack.app"` |

macOS 15 起 Apple 移除了「右键 → 打开」的绕过方式，只能走系统设置里的「仍要打开」。
想彻底免除这一步骤：Developer ID（$99/年）+ 公证（`rcodesign notary-submit` 或 `notarytool`）。

## 7. CI/CD（已实现，见 CI-CD.md）

- 仓库里 `.github/workflows/release.yml`：推 `v*` tag → `npm run tauri build --bundles app` →
  rcodesign 用 secrets 里的 p12 签名 → 打 zip/dmg + SHA256 → 建 Release。
  证书与密码从 repository **secrets** 注入，不需要钥匙串。
- **签名**：rcodesign 是纯 Rust 实现，跨平台可跑（Linux runner 也能签），
  所以签名不绑定 macOS runner；本项目用 macOS runner 只是因为要构建 arm64 二进制。
- **公证**：需要付费 Apple 账号的 App Store Connect API Key（`.p8`）；自签证书**不能公证**
  （公证要求 Apple 签发的身份）。
- 若将来改用 Apple 证书：`security create-keychain` → `security import` → `unlock-keychain`
  → `codesign --keychain`（GitHub 官方 p12 指南的流程，其中不含 `add-trusted-cert`，
  因为 Apple 签发的证书本就受信任）。

## 8. 常见问题与处理方法

| 问题 | 现象 | 处理 |
|---|---|---|
| `security import` 拒绝空密码 p12 | `MAC verification failed during PKCS12 import (wrong password?)` | p12 必须设非空密码 |
| 自签证书未受信任 | `CSSMERR_TP_NOT_TRUSTED` / `codesign: no identity found` | 要么 `add-trusted-cert`，要么改用 rcodesign（本项目选后者） |
| tauri.conf 的 `signingIdentity` | 身份不在钥匙串时，打包签名步骤会失败 | 已移除该字段；打包产出 ad-hoc 签名，随后由 rcodesign 覆盖 |
| `security delete-identity` 的副作用 | 从登录钥匙串删除身份后，独立钥匙串文件里的身份也不再被 `find-identity -v` 视为有效 | 独立钥匙串方案已被 rcodesign 取代，不再需要 |
| 额外二进制文件被打包进 bundle | `Contents/MacOS/` 里出现了 `llm_test2` | 已把 `src/bin/` 下的开发工具移到 `examples/`（`cargo run --example …`） |
| iconset 目录名 | `iconutil` 报 `Invalid Iconset` | 目录名必须以 `.iconset` 结尾 |
| zsh 不做词分割 | `for s in "16 16"; do set -- $s; sips -z $2 $1` 里 `$2` 为空 → sips 静默失败 | 逐条显式写 sips |

## 9. 参考资料

- rcodesign 签名：<https://gregoryszorc.com/docs/apple-codesign/main/apple_codesign_rcodesign_signing.html>
- rcodesign 公证：<https://gregoryszorc.com/docs/apple-codesign/main/apple_codesign_rcodesign_notarizing.html>
- rcodesign GitHub Action：<https://gregoryszorc.com/docs/apple-codesign/main/apple_codesign_github_actions.html>
- Apple TN2206（自签身份与自建 CA；Gatekeeper 只检查带隔离的文件）
- Apple TN3127（指定要求 / TCC 记录的是一段 DR）
- Apple TN3126（签名结构；Apple 明确不要自行实现）
- Apple 论坛 thread 712043（未受信任证书的修复方式）
- GitHub 官方 p12 安装指南（macOS runner 的临时钥匙串流程）
