// 录音浮窗生命周期(用户定则):
//   按住=录音中(逐字滚动) → 松开=转写/润色/交付(不自动消失) → 粘贴成功=立即关闭
//   ASR 失败=浮窗保留(partial 文本不动)+重试按钮; 重试一次仍失败=引导去设置页
//   LLM 失败但 ASR 成功=贴 ASR 原文+显示问题 2.5s 后关
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";

const $ = (id: string) => document.getElementById(id)!;
const statusEl = $("status");
const textEl = $("text");
const fill = $("fill") as HTMLElement;
const dot = $("dot");
const failActions = $("failActions");
const failMsg = $("failMsg");
const retryBtn = $("retryBtn") as HTMLButtonElement;

let retried = false; // hud 只重试一次, 再失败引导去设置页

function setState(cls: string, msg: string, keepText = true) {
  dot.className = "dot " + cls;
  statusEl.textContent = msg;
  if (!keepText) textEl.textContent = "";
  failActions.classList.remove("show");
}

// 暴露给 fail-actions 按钮
(window as any).__retry = async () => {
  if (retried) return;
  retried = true;
  retryBtn.disabled = true;
  failMsg.textContent = "重试中…";
  try {
    await invoke("retry_last");
    // 成功则后续 asr-final 接管; 失败则 asr-error 再次到达
  } catch (e) {
    showRetryFailed(String(e));
  }
};
(window as any).__close = () => invoke("hud_hide");

function showRetryFailed(err: string) {
  setState("err", "⚠️ 转写失败");
  failActions.classList.add("show");
  failMsg.textContent = `重试失败(${err.slice(0, 40)}); 可到 设置→历史 选中记录重跑`;
}

listen<string>("asr-partial", (e) => {
  textEl.textContent = e.payload;
  textEl.scrollTop = textEl.scrollHeight;
});
listen<string>("asr-result", () => {
  setState("idle", "… 处理中");
  fill.style.width = "0";
});
listen<any>("asr-final", (e) => {
  const d = e.payload;
  if (d.warning) {
    // LLM/交付环节有问题但文本已交付(ASR 原文兜底): 显示问题 2.5s 后关
    setState("ok", `✓ 已交付 ⚠️ ${String(d.warning).slice(0, 60)}`);
    textEl.textContent = d.final || d.raw || "";
    setTimeout(() => invoke("hud_hide"), 2500);
  } else {
    // 干净交付: 立即关闭
    invoke("hud_hide");
  }
});
listen<string>("asr-error", (e) => {
  // 用户定则: 浮窗必消失——错误显示 2.5s 自动关(期间可点重试); 重试入口也在历史页
  dot.className = "dot err";
  statusEl.textContent = "⚠️ 转写失败";
  fill.style.width = "0";
  failActions.classList.add("show");
  failMsg.textContent = String(e.payload).slice(0, 60);
  setTimeout(() => invoke("hud_hide"), 2500);
});
listen<number>("asr-level", (e) => {
  const lv = Math.min(1, (e.payload / 1000) * 6);
  fill.style.width = `${Math.max(4, lv * 100)}%`;
  fill.className = lv > 0.9 ? "fill hot" : "fill";
});
listen<string>("hud-state", (e) => {
  if (e.payload === "recording") {
    retried = false; // 新录音重置重试计数
    retryBtn.disabled = false;
    setState("", "● 录音中", false);
    document.getElementById("hint")!.textContent = "松开结束 · esc 取消";
    fill.style.width = "0";
  } else if (e.payload === "transcribing") {
    setState("idle", "… 转写中");
    document.getElementById("hint")!.textContent = "";
    fill.style.width = "0";
  }
});
// 阶段状态: 无自动隐藏——从松开一直显示到 asr-final(交付完成)
listen<string>("hud-busy", (e) => {
  setState("idle", e.payload);
  fill.style.width = "0";
});
listen<string>("hud-msg", (e) => {
  setState("idle", e.payload, false);
  fill.style.width = "0";
  setTimeout(() => invoke("hud_hide"), 2500);
});
