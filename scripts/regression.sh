#!/bin/bash
# 回归闸门: 每次打包前必跑(修 A 坏 B 根治). 任一失败即停.
set -e
cd "$(dirname "$0")/.."
echo "== 1/4 cargo test =="
(cd src-tauri && cargo test --lib 2>&1 | tail -1)
echo "== 2/4 文件回放链路 =="
pkill -x denpa-jack 2>/dev/null; sleep 1
W=tests/fixtures/sample_short_16k.wav
H="$HOME/Library/Application Support/io.github.yangyaofei.denpajack/history.jsonl"
BEFORE=$(wc -l < "$H" 2>/dev/null || echo 0)
timeout 60 env VOICEMAC_AUTOTEST_FILE="$W" src-tauri/target/release/denpa-jack > tmp/reg-file.txt 2>&1
AFTER=$(wc -l < "$H")
[ "$AFTER" -gt "$BEFORE" ] && echo "PASS: history +$((AFTER-BEFORE))" || { echo "FAIL: history 未追加"; tail -20 tmp/reg-file.txt; exit 1; }
grep -a "delivered=" tmp/reg-file.txt | tail -1
echo "== 3/5 UI 数据链(页面同款函数 vs 磁盘) =="
timeout 30 env VOICEMAC_AUTOTEST=ui src-tauri/target/release/denpa-jack 2>&1 | grep -a "ui-chain" | tail -4
echo "== 前端静态检查 =="
(cd .. && npm run build > /dev/null 2>&1 && node tests/frontend-checks.mjs) || exit 1
echo "== 4/5 e2e 真麦 =="
timeout 40 env VOICEMAC_AUTOTEST=e2e src-tauri/target/release/denpa-jack > tmp/reg-e2e.txt 2>&1 || true
echo "== 5/5 打包+签名 =="
npm run tauri build 2>&1 | grep -cE "Bundling Denpa Jack.app"
# 签名用 rcodesign 直接读 p12 文件(不需要钥匙串导入/不需要系统信任; 详见 docs/CODE-SIGNING.md)
# 签名资产在仓库内 certs/（gitignored）
~/.local/bin/rcodesign sign \
  --p12-file "certs/denpa-jack-dev-10y.p12" \
  --p12-password-file "certs/denpa-jack-dev-10y.pw" \
  "src-tauri/target/release/bundle/macos/Denpa Jack.app"
echo "REGRESSION PASS"
