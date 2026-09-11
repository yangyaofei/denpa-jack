// 前端冒烟(对齐 Handy Playwright 冒烟语义, 零依赖 node 实现): dev server 起来+关键 DOM 在
// 用法: node tests/smoke.mjs <port>
const port = process.argv[2] || "5203";
const res = await fetch(`http://localhost:${port}/index.html`);
const html = await res.text();
const checks = [
  ["200", res.status === 200],
  ["html 骨架", html.includes("<html") && html.includes("<body")],
  ["Alpine 挂载点", html.includes("x-data")],
  ["历史双栏", html.includes("pane-left") || html.includes("hist")],
];
let fail = 0;
for (const [name, ok] of checks) {
  console.log(`${ok ? "PASS" : "FAIL"}: ${name}`);
  if (!ok) fail++;
}
process.exit(fail ? 1 : 0);
