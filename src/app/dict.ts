// 词典页域(阶段A 第5步从 main.ts 拆出): 词条增删改查 + 热词预览 + 历史快速加词。
import { invoke } from "@tauri-apps/api/core";
import type { AppThis, DictEntry } from "./types";

export function dictPage(this: AppThis) {
  return {
    dictEdit: null as any,
    dictIsNew: false,
    hotwordsPreview: [] as string[],
    dictSearch: "",
    quickTerm: "",

    /// 星级展示(词典权重 boost 1-3): ★=已点亮, ☆=未点亮
    weightStars(this: AppThis, b: number): string {
      return "★★★".slice(0, Math.max(0, b)) + "☆☆☆".slice(0, Math.max(0, 3 - b));
    },

    filteredDict(this: AppThis): DictEntry[] {
      const d = this.cfg?.dict ?? [];
      const q = this.dictSearch.trim().toLowerCase();
      if (!q) return d;
      return d.filter((e) => e.term.toLowerCase().includes(q) || e.variants.some((v) => v.toLowerCase().includes(q)));
    },
    newDict(this: AppThis) {
      this.dictIsNew = true;
      this.dictEdit = { term: "", variants: "", guard_words: "", boost: 2 };
    },
    editDict(this: AppThis, e: DictEntry) {
      this.dictIsNew = false;
      this.dictEdit = { term: e.term, variants: e.variants.join(", "), guard_words: e.guard_words.join(", "), boost: e.boost };
    },
    async delDict(this: AppThis, e: DictEntry) {
      if (!this.cfg) return;
      this.cfg.dict = this.cfg.dict.filter((x) => x.term !== e.term);
      await this.saveCfg();
      await this.refreshHotwords();
    },
    async saveDict(this: AppThis) {
      if (!this.cfg || !this.dictEdit?.term.trim()) return;
      const entry: DictEntry = {
        term: this.dictEdit.term.trim(),
        variants: this.dictEdit.variants.split(/[,，]/).map((s: string) => s.trim()).filter(Boolean),
        guard_words: this.dictEdit.guard_words.split(/[,，]/).map((s: string) => s.trim()).filter(Boolean),
        boost: Number(this.dictEdit.boost) || 0,
      };
      const i = this.cfg.dict.findIndex((x) => x.term === entry.term);
      if (i >= 0) this.cfg.dict[i] = entry; else this.cfg.dict.push(entry);
      this.dictEdit = null;
      await this.saveCfg();
      await this.refreshHotwords();
    },
    async refreshHotwords(this: AppThis) {
      try { this.hotwordsPreview = await invoke("get_hotwords"); } catch (_) {}
    },
    // 详情面板: 把该条文本放进词典快速添加框(切到词典页)
    addTermFromHist(this: AppThis, h: any) {
      this.quickTerm = (h.final_text || h.raw || "").slice(0, 40);
      this.section = "dict";
    },
    async addQuickTerm(this: AppThis) {
      const t = this.quickTerm.trim();
      if (!t) return;
      if (!this.cfg) return;
      if (this.cfg.dict.some((d: any) => d.term === t)) { this.quickTerm = ""; return; }
      this.cfg.dict.push({ term: t, variants: [], guard_words: [], boost: 3 });
      this.quickTerm = "";
      await this.saveCfg();
      await this.refreshHotwords();
    },
  };
}
