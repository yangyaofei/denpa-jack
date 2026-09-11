#!/bin/bash
# 回归闸门: 每次打包前必跑(修 A 坏 B 根治). 任一失败即停.
set -e
cd "$(dirname "$0")/.."
echo "== 1/4 cargo test =="
(cd src-tauri && cargo test --lib 2>&1 | tail -1)
echo "== 2/4 文件回放链路 =="
pkill -x tauri-app 2>/dev/null; sleep 1
W=/Users/yangyaofei/workspace/research-plan/voice-mac-app/test_input/real_1780279293_6d783cb4-bb15-4e94-a911-c3b7149538e6_16k.wav
H="$HOME/Library/Application Support/com.yangyaofei.tauri-app/history.jsonl"
BEFORE=$(wc -l < "$H" 2>/dev/null || echo 0)
timeout 60 env VOICEMAC_AUTOTEST_FILE="$W" src-tauri/target/release/tauri-app > tmp/reg-file.txt 2>&1
AFTER=$(wc -l < "$H")
[ "$AFTER" -gt "$BEFORE" ] && echo "PASS: history +$((AFTER-BEFORE))" || { echo "FAIL: history 未追加"; tail -20 tmp/reg-file.txt; exit 1; }
grep -a "delivered=" tmp/reg-file.txt | tail -1
echo "== 3/5 UI 数据链(页面同款函数 vs 磁盘) =="
timeout 30 env VOICEMAC_AUTOTEST=ui src-tauri/target/release/tauri-app 2>&1 | grep -a "ui-chain" | tail -4
echo "== 4/5 e2e 真麦 =="
timeout 40 env VOICEMAC_AUTOTEST=e2e src-tauri/target/release/tauri-app > tmp/reg-e2e.txt 2>&1 || true
echo "== 5/5 打包+签名 =="
npm run tauri build 2>&1 | grep -cE "Bundling VoiceInput.app"
codesign --force --deep -s "VoiceInput Dev" src-tauri/target/release/bundle/macos/VoiceInput.app
echo "REGRESSION PASS"
