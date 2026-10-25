# 发布与运维

## 版本纪律

一次发布 = 六个位置版本号一致 + 一次 tag + 一次 Release。

六处：`package.json`、`package-lock.json`（两处：根与 `packages[""]`）、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`（`denpa-jack` 包）、`src-tauri/tauri.conf.json`。

`tests/frontend-checks.mjs` 里有"版本号六处一致"断言，打包前会拦一次。

最低系统版本：`src-tauri/tauri.conf.json` 的 `minimumSystemVersion`，当前 `14.0`（ScreenCaptureKit 要求）。

## 签名（为什么是自签）

- 证书：`Denpa Jack Dev`（自签，10 年），资产在仓库 `certs/`（gitignored）：`denpa-jack-dev-10y.p12` + `.pw`。
- bundle id 固定 `io.github.yangyaofei.denpajack`。
- 签名工具：`~/.local/bin/rcodesign`（纯 Rust，直接读 p12，不需要导入钥匙串）。
- 为什么可行：TCC 按「指定要求」记录 `identifier + certificate leaf`，证书和 bundle id 不变 → 重打包不会重新授权。
- 代价：别人下载会遇 Gatekeeper（自签不满足 `anchor apple generic`），需手动放行。要做到零摩擦只能上 Developer ID + 公证（目前不做）。

```bash
~/.local/bin/rcodesign sign \
  --p12-file "certs/denpa-jack-dev-10y.p12" \
  --p12-password-file "certs/denpa-jack-dev-10y.pw" \
  "src-tauri/target/release/bundle/macos/Denpa Jack.app"
codesign --verify --deep --strict "…/Denpa Jack.app"
```

## 本地打包与装机

```bash
npm run tauri build -- --bundles app
# 本地没有 updater 私钥时，会报 "A public key has been found, but no private key" —— 只影响 updater 签名，app 已生成

osascript -e 'quit app "Denpa Jack"'; sleep 2
rm -rf "/Applications/Denpa Jack.app"
cp -R "src-tauri/target/release/bundle/macos/Denpa Jack.app" /Applications/
xattr -cr "/Applications/Denpa Jack.app"
open "/Applications/Denpa Jack.app"
```

注意：`cp -R` 覆盖不会清除旧包上的扩展属性，必须先 `rm -rf` 再拷贝，最后 `xattr -cr`。

## 正式发布（GitHub Release）

1. 版本号六处升级 → 同步 `docs/SPEC.md` 与 handover（如涉及行为）。
2. 提交（详细 commit message）。
3. `git push origin master`。
4. `git tag vX.Y.Z && git push origin vX.Y.Z`。
5. Release workflow（`.github/workflows/release.yml`）在 `macos-14` 上：构建 → 签名 → 打包 → 生成 `latest.json` → 上传 Release。
6. 跟踪：`gh run watch <run-id> --exit-status`。
7. 核资产：`gh release view vX.Y.Z --json assets`，确认 app.tar.gz + .sig + zip + dmg + latest.json + SHA256SUMS。
8. 回验：下载 app.tar.gz，`codesign --verify --strict`、版本号、`latest.json` 签名长度。

触发器：推 `v*` tag，或在 Actions 页面手动指定 tag。

## 内置更新

- 客户端：`update_check` / `update_install`（关于页卡片，手动确认）。
- 更新包签名：minisign 私钥在 `~/.tauri/denpa-jack-updater.key`（另有备份在 `certs/`），CI 用仓库 secrets `TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。
- 注意：updater 包必须在**签名之后**重新打包，否则更新后应用身份变化、TCC 权限会重新弹（历史 bug）。
- `latest.json` 由 workflow 生成，signature 来自 `.sig` 文件。

## 运维与善后

- 数据目录：`~/Library/Application Support/io.github.yangyaofei.denpajack/`（配置、历史、录音、LLM 日志、截图、app.log）。可自定义 `data_dir`。
- 截图/录音有保留上限（截图 200 张、录音按配置），不要手动清空用户数据。
- 失败发布：不删除已发布的 tag/Release，除非用户明确要求；保留供排查与对比。
- 回滚：装旧版本的 app 包即可；配置向后兼容（新增字段都有 `#[serde(default)]`）。
