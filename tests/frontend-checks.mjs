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
check("Key 池 textarea 绑定 saveCfg()", /@change="saveCfg\(\)"/.test(readFileSync("src/pages/asr.html", "utf8")));

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

process.exit(fail ? 1 : 0);
