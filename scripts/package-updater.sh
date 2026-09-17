#!/usr/bin/env bash
# 打包"应用内更新"产物：把**已代码签名**的 .app 打成 tar.gz，并用 minisign 密钥签名。
#
# 为什么必须单独一步：`tauri build` 会在代码签名之前就生成 updater 的 tar.gz，
# 里面装的是 ad-hoc 签名的 app —— 用它做更新会导致更新后身份变化、TCC 授权全部丢失（实测踩过，见
# docs/SPEC.md §5.1）。所以流程固定为：先 rcodesign 签 .app → 再用本脚本重打包 + 签名。
#
# 用法: scripts/package-updater.sh <版本号> [输出目录（默认 dist-release）]
# 需要环境变量: TAURI_SIGNING_PRIVATE_KEY（私钥内容）、TAURI_SIGNING_PRIVATE_KEY_PASSWORD
set -euo pipefail

V="${1:?用法: scripts/package-updater.sh <版本号> [输出目录]}"
OUT="${2:-dist-release}"
APP="src-tauri/target/release/bundle/macos/Denpa Jack.app"

: "${TAURI_SIGNING_PRIVATE_KEY:?缺少 TAURI_SIGNING_PRIVATE_KEY}"
: "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:?缺少 TAURI_SIGNING_PRIVATE_KEY_PASSWORD}"
[ -d "$APP" ] || { echo "找不到 $APP（先 npm run tauri build -- --bundles app）"; exit 1; }

# 守卫：只接受用本项目证书签过名的 app（否则更新包会把权限身份带跑偏）
# 注意：不要写成 `codesign … | grep -q …` —— grep -q 命中即退出会让 codesign 收到 SIGPIPE，
# 在 set -o pipefail 下整个管道判失败，守卫会误报。
DV="$(codesign -dvv "$APP" 2>&1 || true)"
case "$DV" in
  *"Authority=Denpa Jack Dev"*) ;;
  *)
    echo "错误：$APP 未用 Denpa Jack Dev 签名（Authority 不符），拒绝打包更新产物"
    printf '%s\n' "$DV" | head -5
    exit 1
    ;;
esac

mkdir -p "$OUT"
TAR="$OUT/Denpa.Jack_${V}_aarch64.app.tar.gz"
rm -f "$TAR" "$TAR.sig"

# COPYFILE_DISABLE / --disable-copyfile：避免 macOS 往 tar 里塞 ._ AppleDouble 文件
( cd "$(dirname "$APP")" && COPYFILE_DISABLE=1 tar --disable-copyfile -czf "$OLDPWD/$TAR" "Denpa Jack.app" )

npx tauri signer sign -k "$TAURI_SIGNING_PRIVATE_KEY" -p "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD" "$TAR"

echo "更新产物："
ls -l "$TAR" "$TAR.sig"
