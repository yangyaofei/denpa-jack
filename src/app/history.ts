// 历史页域(阶段A 第5步从 main.ts 拆出): 列表/过滤/重跑/清空/打开音频。
import { invoke } from "@tauri-apps/api/core";
import type { AppThis, HistoryRecord } from "./types";

export function historyPage(this: AppThis) {
  return {
    history: [] as HistoryRecord[],
    selHist: null as HistoryRecord | null,
    histSearch: "",

    // ── 历史 ──
    async refreshHistory(this: AppThis) {
      // E: 详情是快照——列表刷新时按 ts 重找, 保证重转/重跑后详情同步
      if (this.selHist) {
        const fresh = (this.history || []).find((h: any) => h.ts === this.selHist.ts);
        if (fresh) this.selHist = fresh;
      }
      try { this.history = await invoke("get_history", { limit: 500 }); }
      catch (e) { console.error("get_history:", e); }
    },
    filteredHist(this: AppThis) {
      const q = this.histSearch.trim().toLowerCase();
      if (!q) return this.history;
      return this.history.filter((h: any) =>
        (h.raw || "").toLowerCase().includes(q) || (h.final_text || "").toLowerCase().includes(q));
    },
    selectHist(this: AppThis, h: HistoryRecord) {
      this.selHist = h;
      // 切条目时右栏详情滚回顶部——否则上一条的滚动位置留着,
      // 新条目的"最终结果/重转按钮"在顶上, 还得手动滑回去(用户反馈)
      this.$nextTick(() => { const el = (this.$refs as any).paneRight; if (el) el.scrollTop = 0; });
    },
    async copyHist(this: AppThis, h: HistoryRecord) {
      try { await invoke("copy_text", { text: h.final_text }); } catch (e) { console.error(e); }
    },
    async openAudio(this: AppThis, h: HistoryRecord) {
      if (!h.audio_path) return;
      try {
        const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
        await revealItemInDir(h.audio_path);
      } catch (e) { console.error(e); }
    },
    async playHist(this: AppThis, h: HistoryRecord) {
      if (!h.audio_path) return;
      try {
        const { openPath } = await import("@tauri-apps/plugin-opener");
        await openPath(h.audio_path);
      } catch (e) { console.error(e); }
    },
    async rerunHist(this: AppThis, h: HistoryRecord) {
      if (!h.audio_path) { this.error = "该记录无音频, 无法重转"; return; }
      if (this.rerunning.has(h.ts)) return;
      try {
        // R1: 后台重算回写条目(不交付不弹 HUD)。"进行中"以后端 core 的
        // rerun_active 为准(轮询镜像到 this.rerunning), 前端不做本地判定
        await invoke("rerun_history", { ts: h.ts, audioPath: h.audio_path });
        this.rerunning.add(h.ts);
        this.error = "";
      } catch (e: any) { this.error = String(e); }
    },
  };
}
