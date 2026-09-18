// C46 回归: 前端轮询调用链静态断言(模块作用域裸调组件方法=ReferenceError 被吞=永不刷新)
// 用法: node tests/frontend-checks.mjs  (需先 npm run build)
import { readFileSync, existsSync, readdirSync } from "node:fs";
import { join } from "node:path";

let fail = 0;
const check = (name, ok) => { console.log(`${ok ? "PASS" : "FAIL"}: ${name}`); if (!ok) fail++; };

// 1) 构建产物存在且含轮询
const distDir = "dist/assets";
check("dist/assets 存在(先 npm run build)", existsSync(distDir));
const mainJs = readdirSync(distDir).find(f => f.startsWith("main-") && f.endsWith(".js"));
check("main-*.js 产物存在", !!mainJs);
if (mainJs) {
  const js = readFileSync(join(distDir, mainJs), "utf8");
  check("轮询已构建进产物(poll_versions)", js.includes("poll_versions"));
  check("心跳已构建进产物(poll alive)", js.includes("poll alive"));
}

// 2) 源码作用域断言: 组件方法不得在模块顶层(深度<=1)被裸调
const src = readFileSync("src/main.ts", "utf8");
// 去字符串/模板串/注释, 防误计大括号
const cleaned = src
  .replace(/\/\*[\s\S]*?\*\//g, "")
  .replace(/\/\/.*$/gm, "")
  .replace(/`(?:[^`\\]|\\.)*`/g, "``")
  .replace(/"(?:[^"\\]|\\.)*"/g, '""')
  .replace(/'(?:[^'\\]|\\.)*'/g, "''");
const methods = ["refreshHistory", "loadCfg", "refreshHotwords", "saveCfg"];
const depthAt = (idx) => {
  let d = 0;
  for (let i = 0; i < idx; i++) {
    const c = cleaned[i];
    if (c === "{") d++;
    else if (c === "}") d--;
  }
  return d;
};
let bareCalls = 0;
for (const m of methods) {
  const re = new RegExp(`(?<![\\w.])(?<!async )(?<!function )${m}\\(`, "g");
  let match;
  while ((match = re.exec(cleaned)) !== null) {
    if (depthAt(match.index) <= 1) {
      bareCalls++;
      console.log(`  违规: 模块顶层裸调 ${m}() @index ${match.index}`);
    }
  }
}
check("组件方法无模块顶层裸调(C46 根因)", bareCalls === 0);

// 2.5) 死事件监听残留(C43: 事件通道不可达, listen 即死代码)
for (const f of ["src/main.ts", "src/hud.ts"]) {
  const t = readFileSync(f, "utf8");
  check(`${f} 无 listen 残留`, !/\blisten\s*</.test(t));
}

// 2.6) 组件方法名统一(别名已删, 调用点必须用规范名; 写错名字=Alpine 表达式错误)
const pageFiles = readdirSync("src/pages").filter((f) => f.endsWith(".html"));
const htmlAll = ["index.html", "hud.html", ...pageFiles.map((f) => `src/pages/${f}`)]
  .map((f) => readFileSync(f, "utf8"))
  .join("\n");
check("无 toggleRec 残留(统一 toggleRecording)", !/\btoggleRec\b/.test(src) && !/\btoggleRec\b/.test(htmlAll));
check("无 saveKeys 残留(统一 saveCfg)", !/\bsaveKeys\b/.test(src) && !/\bsaveKeys\b/.test(htmlAll));
check("试音按钮调用 toggleRecording()", /@click="toggleRecording\(\)"/.test(htmlAll));
// Key 池卡片已删除(issue #5): 页面上不再有"Key 池"字样, 也不再有 keysText 绑定
check(
  "ASR 页已无 Key 池卡片(issue #5)",
  !/Key 池/.test(readFileSync("src/pages/asr.html", "utf8")) && !/\bkeysText\b/.test(src),
);

// 2.7) hud 重试失败文案指向主窗历史(独立设置窗已废除, 历史在主窗侧栏)
const hudSrc = readFileSync("src/hud.ts", "utf8");
check("hud 重试失败指向主窗历史", /主窗/.test(hudSrc) && !/设置→历史/.test(hudSrc));

// 2.8) TS 接口与 Rust settings.rs 字段对齐(settings.rs 为唯一事实来源)
const rs = readFileSync("src-tauri/src/settings.rs", "utf8");
const rsFields = (name) => {
  const body = rs.match(new RegExp(`pub struct ${name} \\{([\\s\\S]*?)\\n\\}`))?.[1] ?? "";
  return [...body.matchAll(/pub\s+(\w+)\s*:/g)].map((m) => m[1]);
};
const tsBody = (name) => src.match(new RegExp(`export interface ${name} \\{([\\s\\S]*?)\\n\\}`))?.[1] ?? "";
for (const name of ["Config", "LlmProfile", "AsrProfile", "DictEntry"]) {
  const body = tsBody(name);
  const missing = rsFields(name).filter((f) => !new RegExp(`\\b${f}\\??\\s*:`).test(body));
  check(`TS ${name} 未遗漏 settings.rs 字段(缺: ${missing.join(",") || "无"})`, missing.length === 0);
}
const deadFields = ["hotkey_key_code", "filler_word_removal", "append_trailing_space"];
check(`无废弃字段残留(${deadFields.join("/")})`, deadFields.every((f) => !src.includes(f)));

// 3) Rust 命令注册
const lib = readFileSync("src-tauri/src/lib.rs", "utf8");
check("poll_versions 已注册 invoke_handler", /commands::poll_versions/.test(lib));
// 关于页版本号走我们自己的命令(不走 JS API 动态 chunk): 注册与前端调用两侧都要在
check("app_version 已注册 invoke_handler", /commands::app_version/.test(lib));
check("update_check 已注册 invoke_handler", /update::update_check/.test(lib));
check("update_install 已注册 invoke_handler", /update::update_install/.test(lib));
check("关于页有应用内更新卡", /应用内更新/.test(readFileSync("src/pages/about.html", "utf8")));
check(
  "tauri.conf 配好 updater(端址+公钥)",
  /"updater"/.test(readFileSync("src-tauri/tauri.conf.json", "utf8")) &&
    /"pubkey"/.test(readFileSync("src-tauri/tauri.conf.json", "utf8")) &&
    /createUpdaterArtifacts/.test(readFileSync("src-tauri/tauri.conf.json", "utf8")),
);
check("前端用 app_version 取版本", /invoke<string>\("app_version"\)/.test(readFileSync("src/main.ts", "utf8")));
check(
  "关于页版本为运行时渲染(非硬编码)",
  /x-text="version/.test(readFileSync("src/pages/about.html", "utf8")),
);

// 4) 分段队列(HUD 队列)静态断言
// 用户定则: 不显示段号/"已交付"字样、队列按需展开、失败可重试、清空两步确认、不丢段
const hudTs = readFileSync("src/hud.ts", "utf8");
const hudHtml = readFileSync("hud.html", "utf8");
const segQ = readFileSync("src-tauri/src/segment_queue.rs", "utf8");
const queueCmds = ["queue_retry", "queue_clear", "queue_dismiss"];
check(
  `队列三命令已注册 invoke_handler(${queueCmds.join("/")})`,
  queueCmds.every((c) => new RegExp(`commands::${c}`).test(lib)),
);
check(
  `hud.ts 调用三命令(${queueCmds.join("/")})`,
  queueCmds.every((c) => hudTs.includes(`invoke("${c}")`)),
);
// 注释里可能写"不显示已交付"这样的规则说明; 断言要看去掉注释后的真实代码/标记
const stripComments = (s) => s.replace(/\/\*[\s\S]*?\*\//g, "").replace(/\/\/.*$/gm, "");
check(
  "hud 不显示段号/已交付字样(用户定则)",
  !/已交付/.test(stripComments(hudTs)) &&
    !/已交付/.test(stripComments(hudHtml)) &&
    !/第\s*\$\{/.test(stripComments(hudTs)),
);
check("hud.html 队列按需展开(hasqueue 才 664px)", /\.hud\.hasqueue\s*\{\s*width:\s*664px/.test(hudHtml));
check("hud.html 当前文字可多行且有滚动上限(110px, 与队列化之前一致)", /max-height:\s*110px/.test(hudHtml) && /cur-text/.test(hudHtml));
// 存活规则(issue #3/#4): 正常完成 350ms 收起; 带警告或只剩失败 2500ms;
// failed 不得进入 busy 判定(否则永不收起)
check(
  "hud.ts 存活规则: 完成 350ms / 警告与只剩失败 2500ms(统一排程一次)",
  /hideAfter = d\.warning \? 2500 : 350/.test(hudTs) &&
    /hideAfter = failed \? 2500 : 350/.test(hudTs) &&
    /if \(!busy && hideAfter !== null\) scheduleHide\(hideAfter\)/.test(hudTs) &&
    !/\|\| failed/.test(hudTs),
);
check(
  "浮窗只改宽度(480↔664), 不按内容改高",
  /invoke\("hud_resize", \{ width: w \}\)/.test(stripComments(hudTs)) &&
    !/getBoundingClientRect/.test(stripComments(hudTs)) &&
    !/resize_window/.test(readFileSync("src-tauri/src/overlay.rs", "utf8")),
);
check("segment_queue.rs 在 lib.rs 启动 worker", /segment_queue::start\(/.test(lib));
check(
  "队列不丢段: 失败/清空都先写历史",
  /delivered: "failed"/.test(segQ) && /delivered: "cancelled"/.test(segQ),
);
// 串行机制: 单 worker 用 Condvar 等 Queued 段, 一次只把一段置为 Transcribing(交付顺序=FIFO)
check(
  "队列串行保证顺序(单 worker + Condvar)",
  /Condvar/.test(segQ) && /CV\.wait\(/.test(segQ) && /state = SegState::Transcribing/.test(segQ),
);

// 5) 版本号清单一致性
// 发版时六处必须同号(package.json / package-lock.json 顶层 / package-lock.json packages[""] /
// Cargo.toml / Cargo.lock / tauri.conf.json); 漏一处会让包版本、关于页显示、更新判断错位
const pkgV = JSON.parse(readFileSync("package.json", "utf8")).version;
const lockRaw = readFileSync("package-lock.json", "utf8");
const lockV = JSON.parse(lockRaw).version;
const lockRootV = JSON.parse(lockRaw).packages[""].version;
const cargoV = readFileSync("src-tauri/Cargo.toml", "utf8").match(/^version = "([^"]+)"/m)?.[1];
const confV = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8")).version;
const cargoLockV = readFileSync("src-tauri/Cargo.lock", "utf8").match(/name = "denpa-jack"\nversion = "([^"]+)"/)?.[1];
const allV = [pkgV, lockV, lockRootV, cargoV, confV, cargoLockV];
check(`版本号六处一致(${allV.join("/")})`, allV.every((v) => v && v === allV[0]));

// 6) issue #6: 空转写(没听清)不能当失败常驻
check(
  "空转写走 NoSpeech(不进失败队列)",
  /NoSpeech/.test(readFileSync("src-tauri/src/doubao.rs", "utf8")) &&
    /AsrEvent::NoSpeech/.test(readFileSync("src-tauri/src/engines.rs", "utf8")) &&
    /record_no_speech/.test(segQ),
);
check(
  "失败提示 2.5s 后自动收走(queue_dismiss)",
  /failTimer/.test(hudTs) && /queue_dismiss/.test(hudTs) && /2500\)/.test(hudTs),
);
check(
  "录音中不显示失败段与失败条",
  /recording \? qAll\.filter/.test(hudTs) && /showFail\(failed && !recording\)/.test(hudTs),
);
// 7) issue #9: 空转写必须结束阶段(否则前端按"转写中"判定为忙 → 永不收起);
//    静音提醒阈值必须贴着 0(该麦克风正常说话 peak 只有 0.06, 阈值过高会误报);
//    录音中的一次性提示不参与收窗
check(
  "空转写走 no_speech 并结束阶段",
  /apply_no_speech/.test(readFileSync("src-tauri/src/hud.rs", "utf8")) &&
    /apply_no_speech\(g, &r\)/.test(readFileSync("src-tauri/src/hud.rs", "utf8")) &&
    /crate::hud::no_speech\(&app, &payload\)/.test(readFileSync("src-tauri/src/engines.rs", "utf8")),
);
check("静音阈值贴 0(避免正常说话误报)", /if lv < 2 \{/.test(readFileSync("src-tauri/src/recording.rs", "utf8")));
check("录音中的提示不触发收窗(静音提醒走 msg 但不排程收窗)", /if \(!recording\) hideAfter = 2500/.test(hudTs));

process.exit(fail ? 1 : 0);
