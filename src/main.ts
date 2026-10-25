// 主窗入口(阶段A 第5步按页拆分): 组件逻辑住在 src/app/*.ts, 这里只做组合与启动。
// 组合模式: 每页导出 partial(状态+方法), 展开后即为原来那个完整的 Alpine 组件对象;
// 跨页调用走 this(AppThis 类型保检查), HTML 零改动。
import "./style.css";
import pageAsr from "./pages/asr.html?raw";
import pageHistory from "./pages/history.html?raw";
import pageGeneral from "./pages/general.html?raw";
import pageDict from "./pages/dict.html?raw";
import pageLlm from "./pages/llm.html?raw";
import pageAbout from "./pages/about.html?raw";

import Alpine from "alpinejs";
import { common } from "./app/common";
import { asrPage } from "./app/asr";
import { llmPage } from "./app/llm";
import { dictPage } from "./app/dict";
import { historyPage } from "./app/history";
import { generalPage } from "./app/general";
import { aboutPage } from "./app/about";

// app() 全局 Alpine 组件
(window as any).app = () => ({
  ...common(),
  ...historyPage(),
  rerunning: new Set<string>() as Set<string>,
  ...dictPage(),
  ...generalPage(),
  ...asrPage(),
  ...llmPage(),
  ...aboutPage(),
});

// 页面片段注入(B47 拆分: index 骨架 + src/pages/*.html)
const pageHtml: Record<string, string> = { asr: pageAsr, history: pageHistory, general: pageGeneral, dict: pageDict, llm: pageLlm, about: pageAbout };
for (const [name, html] of Object.entries(pageHtml)) {
  const slot = document.querySelector(`[data-page="${name}"]`);
  if (slot) { slot.innerHTML = html; slot.removeAttribute("data-page"); }
}
// 实测(2026-09-15): 同一个窗口里 app() 被求值两次 → 两个组件实例, 轮询与 [ui] 日志成对出现。
// 第二次实例的来源没有定位到, 这里加两道幂等守卫: 每个窗口只启动一次 Alpine, 且只初始化一次。
const _win = window as any;
if (!_win.__denpaAlpineStarted) {
  _win.__denpaAlpineStarted = true;
  Alpine.start();
}
