# CI/CD（GitHub Actions）

两条流水线，都在 `.github/workflows/`：

| 工作流 | 触发 | 做什么 |
|---|---|---|
| `ci.yml` | push 到 `master`、PR、手动 | `npm ci` → 前端构建 → 前端静态检查 → `cargo test --lib` → `cargo check`（build.rs 契约闸） |
| `release.yml` | 推 `v*` tag、或手动指定 tag | 注入版本号 → 打包 `.app` → 用 rcodesign 签名 → 打 zip/dmg + SHA256 → 建 GitHub Release |

CI 不跑需要真实密钥与本机权限的步骤（文件回放、e2e 真麦、真机交付）；那些用本地 `./scripts/regression.sh` 跑。

## 需要配置的仓库凭据（GitHub → Settings → Secrets and variables → Actions）

| 名称 | 类型 | 内容 | 怎么生成 |
|---|---|---|---|
| `APPLE_CERT_P12_BASE64` | **Secret** | 证书 `.p12` 的 base64 文本 | `base64 -i certs/denpa-jack-dev-10y.p12 \| pbcopy`（或 `base64 -i … > p12.b64`） |
| `APPLE_CERT_PASSWORD` | **Secret** | `.p12` 的密码（即 `certs/denpa-jack-dev-10y.pw` 的内容） | 见 `docs/CODE-SIGNING.md` 的证书生成一节 |
| `TAURI_SIGNING_PRIVATE_KEY` | **Secret** | 应用内更新包的 minisign 私钥（`certs/denpa-jack-updater.key` 的内容） | `gh secret set TAURI_SIGNING_PRIVATE_KEY < certs/denpa-jack-updater.key` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | **Secret** | 该私钥的口令（`certs/updater-signing.pw`） | `gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD < certs/updater-signing.pw` |

> **重要**：这四个值都必须放 **Secrets**，不要放 **Variables**。Variables 在 public 仓库里是明文可见的，
> 等于把私钥密码公开；Secrets 是加密存储、只在运行时注入。
> 仓库里也可以建一个普通 Variable 记录证书名（如 `APPLE_CERT_CN=Denpa Jack Dev`）作为备忘，但那不是必须的。
>
> **更新签名私钥与代码签名证书是两套凭据**：前者是 `tauri-plugin-updater` 的 minisign 密钥（校验更新包来源，无法关闭），
> 后者是 macOS 的代码签名证书（决定 app 能不能跑、TCC 授权是否延续）。两者都要备份，丢了都会导致各自链路失效。

## 发一个版本

```bash
# 方式一：推 tag（CI 自动构建+签名+建 Release）
git tag v0.2.0 && git push origin v0.2.0

# 方式二：在 Actions 页面手动 Run workflow，填 tag（例如 v0.2.0）
```

工作流会用 tag 里的版本号覆盖 `package.json` / `src-tauri/tauri.conf.json` / `src-tauri/Cargo.toml` 的版本，
再打包——所以不需要手动改版本号。

Release 产物：

| 文件 | 说明 |
|---|---|
| `Denpa.Jack_<版本>_aarch64.dmg` | 磁盘映像（拖进「应用程序」即可） |
| `Denpa.Jack_<版本>_aarch64.app.zip` | 应用包（`ditto` 打包，保留符号链接与签名） |
| `Denpa.Jack_<版本>_aarch64.app.tar.gz` + `.sig` | **应用内更新用**的归档与其 minisign 签名 |
| `latest.json` | **应用内更新清单**（版本/说明/发布时间/`darwin-aarch64` 的签名与下载地址） |
| `SHA256SUMS_<版本>.txt` | 校验和 |

`latest.json` 由 release.yml 用 `jq` 生成：签名值取 `.sig` 文件全文（`jq` 负责转义换行），
下载地址指向同一次 Release 的 `*.app.tar.gz`。应用内更新器轮询的是
`https://github.com/<owner>/<repo>/releases/latest/download/latest.json` ——
`/releases/latest/` 只认非预发布的最新 Release，所以**发版不要勾 prerelease**，否则更新不会被发现。

## 应用内更新（为什么它比"去网页下载"更适合本机）

浏览器下载的文件会带 `com.apple.quarantine`，macOS 按"来自互联网的未受信任 App"处理，会重新索要权限；
应用自己下载不带该标记（未声明 `LSFileQuarantineEnabled`），且新二进制满足同一份代码签名要求，
因此**升级后 TCC 授权保持不变**。机制细节见 `docs/SPEC.md` §5.1。

## 版本与权限的关系

- 签名身份（证书 + bundle id `io.github.yangyaofei.denpajack`）在版本之间保持不变，
  所以升级不会重置 macOS 的权限授权（TCC 按签名要求记录，见 `docs/CODE-SIGNING.md`）。
- 证书有效期 10 年；要换证书时：生成新 p12 → 更新上面两个 Secret → 重新发一版；
  换证书后本机需要重新授权一次（权限绑定签名身份）。

## 为什么 CI 能签名（而不需要钥匙串与系统信任）

rcodesign 直接从 `.p12` 读私钥并生成 Apple 格式的签名，不导入钥匙串、不要求证书被系统信任——
这正是它比 `codesign` 更适合 CI 的原因（`codesign` 在未受信任的证书上会报 `no identity found`，
证据见 `docs/CODE-SIGNING.md`）。rcodesign 是纯 Rust 实现，跨平台可跑，Linux runner 也能签。

## 别人拿到 Release 之后

自签名的应用在别人机器上仍会被 Gatekeeper 拦（提示"无法验证开发者"）：需要在
系统设置 → 隐私与安全性 → 「仍要打开」，或 `xattr -dr com.apple.quarantine`。
零摩擦分发需要 Apple Developer ID（付费）+ 公证，那是另一条路（见 `docs/CODE-SIGNING.md`）。
