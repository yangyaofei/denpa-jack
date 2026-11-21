// 历史页域(阶段A 第5步从 main.ts 拆出): 列表/过滤/重跑/清空/打开音频。
import { invoke } from "@tauri-apps/api/core";
import type { AppThis, HistoryRecord } from "./types";

export function historyPage() {
  return {
    history: [] as HistoryRecord[],
    selHist: null as HistoryRecord | null,
    histSearch: "",
    histFilter: "all" as "all" | "llm" | "warn",

    // ── 历史 ──
    async refreshHistory(this: AppThis) {
      // E: 详情是快照——列表刷新时按 ts 重找, 保证重转/重跑后详情同步
      const sel = this.selHist;
      if (sel) {
        const fresh = (this.history || []).find((h: any) => h.ts === sel.ts);
        if (fresh) this.selHist = fresh;
      }
      try { this.history = await invoke("get_history", { limit: 500 }); }
      catch (e) { console.error("get_history:", e); }
    },
    filteredHist(this: AppThis) {
      const q = this.histSearch.trim().toLowerCase();
      return this.history.filter((h: any) => {
        if (this.histFilter === "llm" && !h.llm_used) return false;
        if (this.histFilter === "warn" && !h.warning) return false;
        if (!q) return true;
        return (h.raw || "").toLowerCase().includes(q) || (h.final_text || "").toLowerCase().includes(q);
      });
    },
    deliveryLabel(this: AppThis, d: string): string {
      if (d === "pasted-ax" || d === "clipboard+paste") return "AX 直写";
      if (d === "pasted-cmdv") return "Cmd+V 粘贴";
      if (d === "copied" || d === "copied-self" || d === "focus-changed-copied-only") return "仅复制";
      if (d === "failed") return "失败";
      if (d === "cancelled") return "已取消";
      return d || "未知";
    },
    histStats(this: AppThis) {
      const h = this.history || [];
      return {
        total: h.length,
        llmCount: h.filter((x: any) => x.llm_used).length,
        pastedCount: h.filter((x: any) => (x.delivered || "").includes("paste")).length,
      };
    },
    selectHist(this: AppThis, h: HistoryRecord) {
      this.selHist = h;
      // 切条目时右栏详情滚回顶部——否则上一条的滚动位置留着,
      // 新条目的"最终结果/重转按钮"在顶上, 还得手动滑回去(用户反馈)
      this.$nextTick(() => { const el = (this.$refs as any).paneRight; if (el) el.scrollTop = 0; });
    },
    async copyHist(this: AppThis, h: HistoryRecord) {
      await this.copyWithFeedback("hist-" + h.ts, h.final_text || h.raw);
    },
    async openAudio(this: AppThis, h: HistoryRecord) {
      if (!h.audio_path) return;
      try {
        await invoke("reveal_audio", { path: h.audio_path, reveal: true });
      } catch (e: any) { this.error = String(e); }
    },
    async playHist(this: AppThis, h: HistoryRecord) {
      if (!h.audio_path) return;
      try {
        await invoke("reveal_audio", { path: h.audio_path, reveal: false });
      } catch (e: any) { this.error = String(e); }
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
