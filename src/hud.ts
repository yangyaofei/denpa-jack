// 录音浮窗（分段队列方案 D —— 与 src-tauri/src/pipeline/segment_queue.rs、src-tauri/src/ui/hud.rs 同源）
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
const bar = $("bar") as HTMLElement;
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
  hud.dataset.phase = cls || "recording";
  statusEl.textContent = msg;
  if (!keepText) curText.textContent = "";
  failActions.classList.remove("show");
}

// 宽度切换: 单栏 480 ↔ 出现队列时 664。只上报宽度——
// 高度固定(窗口 200), 长文本在框内滚动; 位置由"按下时定位一次"决定, 之后再不动。
// (原版 hud_resize 在队列化之前是死代码、且按高度改窗口会让位置漂, 已删)
let lastWidth = 0;
function pushWidth() {
  const w = hud.classList.contains("hasqueue") ? 664 : 480;
  if (w === lastWidth) return;
  lastWidth = w;
  invoke("hud_resize", { width: w }).catch(() => {});
}

// 右栏: 队列列表(只有状态圆点/旋转环 + 文字; 失败行标红)
let queueSig = "";
function renderQueue(rows: any[]) {
  // 键控签名: 状态 + 文本; 一样就不动 DOM(避免 150ms 一次的重绘打断滚动/闪动)。
  // 重转段已在快照层被 HUD 过滤(展示端自决: 录音浮窗只管语音链路的段),
  // 这里不会收到 source==="rerun" 的行
  const sig = rows.map((r) => `${r.state}|${r.text}`).join("\n");
  if (sig === queueSig) return;
  queueSig = sig;
  qlist.innerHTML = rows
    .map((r) => {
      // 失败段可能没有文本(ASR 阶段就失败, 没有原文) → 给一个可读短标签,
      // 否则那一行只剩一个红点, 用户看不出发生了什么(issue #3)
      const raw = String(r.text || "");
      const label = raw || (r.state === "failed" ? "识别失败" : "");
      const text = label.replace(/[<>&]/g, (c: string) => ({ "<": "&lt;", ">": "&gt;", "&": "&amp;" }[c]!));
      return `<div class="qrow ${r.state}"><span class="b"></span><span class="t">${text}</span></div>`;
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

    // ── 状态阶段(从数据判, 不再由 Rust 侧生成展示态) ──
    const cur = s.cur;
    const curState = cur ? String(cur.state || "") : "";
    const recording = !!s.recording || curState === "recording";
    curText.dataset.rec = recording ? "1" : "0";
    const lv = Math.min(1, (s.level / 1000) * 6);
    bar.style.display = recording ? "" : "none";
    fill.style.width = recording ? `${Math.max(6, lv * 100)}%` : "0";
    fill.className = recording && lv > 0.9 ? "fill hot" : "fill";

    // 收窗时长由"本轮发生的事实"决定一次。
    let hideAfter: number | null = null;

    // 状态行: 录音 > 润色(通知) > 转写中(当前段) > 空
    if (recording) {
      setState("", "录音中", false);
    } else if (s.polishing) {
      setState("polishing", "AI 润色中…");
    } else if (curState === "transcribing" || curState === "queued") {
      setState("idle", "转写中");
    } else {
      setState("", "", false);
    }
    if (s.msg) {
      setState("warn", s.msg, false);
      // 一次性提示(如"没听清(转写为空)"/"录音太短已丢弃")留 2.5s;
      // 录音中不排程(静音提醒也走 msg, 但录音还在进行, 收窗由阶段决定)。
      if (!recording) hideAfter = 2500;
    }

    // 左栏: 当前段(=用户最后说的那条; 录音中 text 就是流式文字, 始终滚到最新)
    const curStr = cur ? String(cur.text || "") : "";
    if (curStr || curState !== "transcribing") {
      if (curText.textContent !== curStr) curText.textContent = curStr;
    }
    if (recording) curText.scrollTop = curText.scrollHeight;

    // 右栏: 其余段(空 → 不显示右栏, 面板收回单栏宽度)
    // 录音中不展示失败段(用户定则: 录音时不该被上一段的失败打扰; 录完再说)
    const qAll: any[] = s.queue || [];
    const q = recording ? qAll.filter((r) => r.state !== "failed") : qAll;
    renderQueue(q);
    hud.classList.toggle("hasqueue", q.length > 0);
    pushWidth(); // 只改宽度(480↔664); 高度固定, 位置不动
    clearBtn.classList.toggle("show", q.some((r) => r.state === "queued"));

    // 失败提示 + 存活规则(录音中不弹失败条, 录完再提示)
    const failed = !!s.failed;
    showFail(failed && !recording);
    if (s.err) {
      // 引擎/交付级错误(asr-error): 同样只在没录音时提示
      setState("err", String(s.err).slice(0, 60));
      showFail(!recording);
      if (!recording) hideAfter = 2500;
    }
    // 存活规则(用户定则 2026-11-05 重申):
    //   浮窗消失的唯一前提 = 正在转写的所有内容都已**成功交付**(交付完成起等 N 秒收起)。
    //   - 活段(录音/引擎在途/排队/结算中)不算完 → 不收
    //   - 失败段(未成功交付)也不算完 → 不收, 留着显示失败条等用户重试/清空
    //   - rerun 不算: 数据侧已滤, 走到这里的段全是语音链路段
    const active = (r: any) => r.state === "recording" || r.state === "transcribing" || r.state === "queued";
    const queueBusy = q.some(active);
    const curBusy = curState !== "" && curState !== "failed";
    const busy = recording || curBusy || queueBusy || (!recording && failed);
    if (busy) {
      cancelHide(); // 还有活/有失败未处理 → 不自动收
    } else if (s.finished) {
      // 规则 2: 全部交付完成 → 正常完成 350ms 收起;
      //         带 warning(含"未识别到语音内容")要留 2.5s, 否则一闪而过看不见(issue #4)
      const d = s.finished;
      const sig = JSON.stringify(d);
      if (sig !== lastFinal) {
        lastFinal = sig;
        setState(d.warning ? "warn" : "ok", d.warning ? String(d.warning).slice(0, 44) : (d.delivered === "copied" ? "已复制" : "已粘贴"));
        hideAfter = d.warning ? 2500 : 350;
      }
    } else if (s.version !== 0) {
      // 兜底: 无活段/无失败/无交付载荷的静默态(如启动初期), 350ms 收
      hideAfter = 350;
    }
    // 统一排程一次(见本轮开头 hideAfter 的说明)
    if (!busy && hideAfter !== null) scheduleHide(hideAfter);
  } catch {
    // poll 失败静默(窗口隐藏期间)
  }
}
setInterval(pollOnce, 150);
pollOnce();
