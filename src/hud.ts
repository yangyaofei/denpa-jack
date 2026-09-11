// 录音浮窗生命周期(用户定则):
//   按住=录音中(逐字滚动) → 松开=转写/润色/交付(不自动消失) → 粘贴成功=立即关闭
//   ASR 失败=浮窗保留(partial 文本不动)+重试按钮; 重试一次仍失败=引导去设置页
//   LLM 失败但 ASR 成功=贴 ASR 原文+显示问题 2.5s 后关
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
;;;;;;
// 阶段状态: 无自动隐藏——从松开一直显示到 asr-final(交付完成);;

// C43 诊断: 前端事件监听注册完成落日志(Rust 侧正常但前端无反应时, 由此定位断联)
invoke("ui_log", { msg: "hud listeners ready" }).catch(() => {});
let partialCount = 0;;

// C43 A/B 实验: 同事件带显式 AnyLabel target 再注册, 收到则打日志(区分默认 target 与定向 target 的路由差异);

// C43b 轮询模式: 事件通道(Rust→webview)在本机打包版不可达(Any/AnyLabel 均不达, emit 返回 Ok)。
// 前端改为 150ms 拉取快照驱动状态机——invoke 通道已证可靠。
let pollVer = -1;
let hideTimer: number | undefined;
let lastFinal = "";
async function pollOnce() {
  try {
    const s = await invoke<any>("hud_poll");
    if (s.version === pollVer) return;
    pollVer = s.version;
    // level
    const lv = Math.min(1, (s.level / 1000) * 6);
    fill.style.width = `${Math.max(4, lv * 100)}%`;
    fill.className = lv > 0.9 ? "fill hot" : "fill";
    // C47 幂等全量重绘: 快照=唯一真相, 空=清——杜绝 hide 清快照后 DOM 残留旧内容
    // (旧实现 if(s.partial) 只写不清, 下次 show 瞬间闪现上一次的文本)
    if (s.status === "recording") {
      setState("", "● 录音中", false);
      document.getElementById("hint")!.textContent = "松开结束 · esc 取消";
      fill.style.width = "0";
    } else if (s.status === "transcribing") {
      setState("idle", "… 转写中");
      fill.style.width = "0";
    } else if (s.status === "busy") {
      setState("idle", "✦ AI 润色中…");
    } else {
      // 空状态(idle/隐藏): 状态行+文本全清
      setState("", "", false);
      document.getElementById("hint")!.textContent = "";
    }
    textEl.textContent = s.partial || "";
    if (!s.msg && !s.err) failActions.classList.remove("show");
    if (s.msg) {
      setState("idle", s.msg, false);
      fill.style.width = "0";
      if (hideTimer) clearTimeout(hideTimer);
      hideTimer = window.setTimeout(() => invoke("hud_hide"), 2500);
    }
    if (s.err) {
      setState("idle", `⚠️ ${String(s.err).slice(0, 60)}`);
      failActions.classList.add("show");
      fill.style.width = "0";
      if (hideTimer) clearTimeout(hideTimer);
      hideTimer = window.setTimeout(() => invoke("hud_hide"), 2500);
    }
    // finished
    if (s.finished) {
      const d = s.finished;
      if (JSON.stringify(d) !== lastFinal) {
        lastFinal = JSON.stringify(d);
        // 用户定则: 文字贴出去(交付完成)浮窗立即消失——不搞展示期
        setState("ok", d.warning ? `⚠️ ${String(d.warning).slice(0, 40)}` : `✓ ${d.delivered === "copied" ? "已复制" : "已粘贴"}`);
        textEl.textContent = d.final || d.raw || "";
        setTimeout(() => invoke("hud_hide"), 350);
      }
    }
  } catch {
    // poll 失败静默(窗口隐藏期间)
  }
}
setInterval(pollOnce, 150);
pollOnce();
invoke("ui_log", { msg: "hud poll loop started" }).catch(() => {});
