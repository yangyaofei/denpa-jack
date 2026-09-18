// 录音浮窗(用户定则, 分段队列方案 D —— 与 docs/SPEC.md、src-tauri/src/segment_queue.rs 同源)
//
// 布局:
//   左栏 = 当前(录音中=实时文字; 没在录音时=正在转写的那段), 可换行/多行(最多 8 行后框内滚动)
//   右栏 = 队列(其余排队/失败段, 一行一段, 单行截断)
//   右栏只在真的有排队段时出现, 面板 480px → 664px; 队列空了收回 480px
//   一律不显示段号、"已交付"等字样(用户定则: 多余的字不写)
//
// 存活规则(快照驱动, 150ms 轮询; 用户定稿):
//   1) 录音中 / 队列非空(排队或转写中) → 一直显示, 无超时
//   2) 全部交付完成 → 350ms 后收起(与既有"粘贴成功浮窗立即消失"一致)
//   3) 队列里只剩失败段 → 2.5s 提示后收起(与既有"录音太短已丢弃"提示同长)
//   4) 失败 + 队列非空 → 保持显示; 队列跑空后才开始那 2.5s
//   5) 「清空」两步确认(点一下变"确认清空？"), 只清排队段; 排队段音频与原文已进历史
//      (delivered="cancelled"), 不丢数据; 「关闭」= 不再提示失败段(段已进历史, 可在历史页重跑)
import { invoke } from "@tauri-apps/api/core";

const $ = (id: string) => document.getElementById(id)!;
const hud = $("hud");
const statusEl = $("status");
const curText = $("curText");
const fill = $("fill") as HTMLElement;
const dot = $("dot");
const qlist = $("qlist");
const clearBtn = $("clearBtn") as HTMLButtonElement;
const failActions = $("failActions");
const failMsg = $("failMsg");
const retryBtn = $("retryBtn") as HTMLButtonElement;
const closeBtn = $("closeBtn") as HTMLButtonElement;

// —— 浮窗存活: 所有收起都走这里, 有新动态就取消(规则 1 优先) ——
let hideTimer: number | undefined;
function scheduleHide(ms: number) {
  if (hideTimer) clearTimeout(hideTimer);
  hideTimer = window.setTimeout(() => {
    hideTimer = undefined;
    invoke("hud_hide").catch(() => {});
  }, ms);
}
function cancelHide() {
  if (hideTimer) {
    clearTimeout(hideTimer);
    hideTimer = undefined;
  }
}

function setState(cls: string, msg: string, keepText = true) {
  dot.className = "dot " + cls;
  statusEl.textContent = msg;
  if (!keepText) curText.textContent = "";
  failActions.classList.remove("show");
}

// 尺寸上报: 面板宽(单栏/两栏)与内容高度都告诉 Rust, 由它设置窗口尺寸并重新贴屏
let lastSize = "";
function pushResize() {
  const w = hud.classList.contains("hasqueue") ? 664 : 480;
  const h = Math.ceil(hud.getBoundingClientRect().height);
  const sig = `${w}x${h}`;
  if (sig === lastSize) return;
  lastSize = sig;
  invoke("hud_resize", { width: w, height: h }).catch(() => {});
}

// 右栏: 队列列表(只有状态图标 + 文字; 失败行标红)
let queueSig = "";
function renderQueue(rows: any[]) {
  // 键控签名: 状态 + 文本; 一样就不动 DOM(避免 150ms 一次的重绘打断滚动/闪动)
  const sig = rows.map((r) => `${r.state}|${r.text}`).join("\n");
  if (sig === queueSig) return;
  queueSig = sig;
  qlist.innerHTML = rows
    .map((r) => {
      const icon = r.state === "failed" ? "✕" : r.state === "transcribing" ? "⟳" : "•";
      // 失败段可能没有文本(ASR 阶段就失败, 没有原文) → 给一个可读短标签,
      // 否则那一行只剩一个 ✕, 用户看不出发生了什么(issue #3)
      const raw = String(r.text || "");
      const label = raw || (r.state === "failed" ? "识别失败" : "");
      const text = label.replace(/[<>&]/g, (c: string) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;" }[c]!));
      return `<div class="qrow ${r.state}"><span class="b">${icon}</span><span class="t">${text}</span></div>`;
    })
    .join("");
}

// 失败提示: 一行说明 + 重试/关闭; 文案固定(失败原因在历史与日志里)
// 失败提示: 显示 2.5s 后自动收起, 并把失败段真正移出队列(音频与原文已在历史里, 仍可从历史页重跑)。
// 用户定则: 失败只是一次性提示, 不能常驻——录音时更不该出现(issue #6)。
let failTimer: number | undefined;
function showFail(show: boolean) {
  if (show) {
    failMsg.textContent = "转写失败（音频已存入历史，可重试）";
    failActions.classList.add("show");
    if (!failTimer) {
      failTimer = window.setTimeout(async () => {
        failTimer = undefined;
        try {
          await invoke("queue_dismiss");
        } catch {}
      }, 2500);
    }
  } else {
    failActions.classList.remove("show");
    if (failTimer) {
      clearTimeout(failTimer);
      failTimer = undefined;
    }
  }
}

retryBtn.disabled = false;
retryBtn.onclick = async () => {
  retryBtn.disabled = true;
  failMsg.textContent = "重试中…";
  try {
    await invoke("queue_retry");
    // 成功 → 该段回到队列, 后续快照会把它显示为排队/转写中
  } catch (e) {
    failMsg.textContent = `重试失败(${String(e).slice(0, 40)}); 可到主窗『历史』里重跑`;
  } finally {
    retryBtn.disabled = false;
    cancelHide(); // 用户操作过 → 重新按规则计时
  }
};
closeBtn.onclick = async () => {
  try {
    await invoke("queue_dismiss"); // 只是不再提示; 段已进历史, 可在历史页重跑
  } catch {}
  showFail(false);
};
// 「清空」两步确认: 第一下变成"确认清空？"，2.6s 内再点才真清(防误触)
let clearArmed = false;
clearBtn.onclick = async () => {
  if (!clearArmed) {
    clearArmed = true;
    clearBtn.textContent = "确认清空？";
    clearBtn.classList.add("confirm");
    window.setTimeout(() => {
      if (clearArmed) {
        clearArmed = false;
        clearBtn.textContent = "清空";
        clearBtn.classList.remove("confirm");
      }
    }, 2600);
    return;
  }
  clearArmed = false;
  clearBtn.textContent = "清空";
  clearBtn.classList.remove("confirm");
  try {
    await invoke("queue_clear");
  } catch {}
  cancelHide();
};

// C43b 轮询模式: 事件通道(Rust→webview)在本机打包版不可达, 前端 150ms 拉快照驱动状态机
let pollVer = -1;
let lastFinal = "";
async function pollOnce() {
  try {
    const s = await invoke<any>("hud_poll");
    if (s.version === pollVer) return;
    pollVer = s.version;

    // 电平条: 只有录音中才显示电平; 其它时候归零
    // (bug 修复: 原来只在录音分支里写 0, 非录音态会保留上一次的宽度, 看起来像还在录音——issue #3)
    const recording = s.status === "recording";
    const lv = Math.min(1, (s.level / 1000) * 6);
    fill.style.width = recording ? `${Math.max(4, lv * 100)}%` : "0";
    fill.className = recording && lv > 0.9 ? "fill hot" : "fill";

    // 状态行
    if (recording) {
      setState("", "● 录音中", false);
    } else if (s.status === "transcribing") {
      setState("idle", "… 转写中");
    } else if (s.status === "busy") {
      setState("idle", "✦ AI 润色中…");
    } else {
      setState("", "", false);
    }
    if (s.msg) {
      setState("idle", s.msg, false);
      scheduleHide(2500); // 一次性提示(如"录音太短已丢弃"): 2.5s
    }

    // 左栏: 当前段(录音中显示实时文字, 始终滚到最新)
    // 松手瞬间队列里还没有这一段(ASR 结果要几百毫秒后才到), seg_current 会短暂为空——
    // 此时若把左栏清空, 视觉上就是"回显瞬间消失、结果到了又出现"(用户反馈)。
    // 规则: 转写中且暂无新文本时, 保留上一份回显; 其余状态(录音重新开始/已结束)才清。
    const cur = s.seg_current;
    const curStr = cur ? String(cur.text || "") : "";
    if (curStr || s.status !== "transcribing") {
      if (curText.textContent !== curStr) curText.textContent = curStr;
    }
    if (cur && cur.state === "recording") curText.scrollTop = curText.scrollHeight;

    // 右栏: 队列(空 → 不显示右栏, 面板收回单栏宽度)
    // 录音中不展示失败段(用户定则: 录音时不该被上一段的失败打扰; 录完再说)
    const qAll: any[] = s.seg_queue || [];
    const q = recording ? qAll.filter((r) => r.state !== "failed") : qAll;
    renderQueue(q);
    hud.classList.toggle("hasqueue", q.length > 0);
    clearBtn.classList.toggle("show", q.some((r) => r.state === "queued"));

    // 失败提示 + 存活规则(录音中不弹失败条, 录完再提示 2.5s 后自动收走)
    const failed = !!s.seg_failed;
    showFail(failed && !recording);
    if (s.err) {
      // 引擎/交付级错误(asr-error): 同样只在没录音时提示, 2.5s 后自动收走(与失败段同规则)
      setState("err", `⚠️ ${String(s.err).slice(0, 60)}`);
      showFail(!recording);
      // 2.5s 由 showFail 里的定时器负责; 这里不再额外 scheduleHide(否则可能与收走时序打架)
    }
    // 存活规则(用户定稿):
    //   busy 只算"真的有活在跑"(录音中/转写中/排队中) —— 失败段不算"活",
    //   只剩失败时按规则 3 计 2.5s 后收起(issue #3: 原来把 failed 也算进 busy, 于是永不收起)
    const queueBusy = q.some((r) => r.state === "queued" || r.state === "transcribing");
    const busy = s.status === "recording" || s.status === "transcribing" || (cur && cur.state === "transcribing") || queueBusy;
    if (busy) {
      cancelHide(); // 规则 1/4: 还有段在排队或转写 → 不自动收
    } else if (s.finished) {
      // 规则 2: 全部交付完成 → 正常完成 350ms 收起;
      //         带 warning(含"未识别到语音内容")要留 2.5s, 否则一闪而过看不见(issue #4)
      const d = s.finished;
      const sig = JSON.stringify(d);
      if (sig !== lastFinal) {
        lastFinal = sig;
        setState(d.warning ? "idle" : "ok", d.warning ? `⚠️ ${String(d.warning).slice(0, 40)}` : `✓ ${d.delivered === "copied" ? "已复制" : "已粘贴"}`);
        scheduleHide(d.warning ? 2500 : 350);
      }
    } else if (s.version !== 0) {
      // 规则 3: 只剩失败段 → 2.5s 提示后收起(与"录音太短已丢弃"同长)
      scheduleHide(failed ? 2500 : 350);
    }

    requestAnimationFrame(pushResize);
  } catch {
    // poll 失败静默(窗口隐藏期间)
  }
}
setInterval(pollOnce, 150);
pollOnce();
