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

// 3) Rust 命令注册
const lib = readFileSync("src-tauri/src/lib.rs", "utf8");
check("poll_versions 已注册 invoke_handler", /commands::poll_versions/.test(lib));

process.exit(fail ? 1 : 0);
