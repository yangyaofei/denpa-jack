// LLM 页域(阶段A 第5步从 main.ts 拆出): 润色档案增删改用 + 模型列表 + 自测。
import { invoke } from "@tauri-apps/api/core";
import { uid } from "./util";
import type { AppThis, LlmProfile } from "./types";

export function llmPage(this: AppThis) {
  return {
    llmEdit: null as LlmProfile | null,
    llmIsNew: false,
    llmModels: [] as string[],
    llmModelsError: "",
    selftesting: false,
    selftestResult: "",
    editLlmPromptPreview: false,
    defaultPrompt: "",

    async togglePromptPreview(this: AppThis) {
      if (this.editLlmPromptPreview) {
        this.editLlmPromptPreview = false;
        return;
      }
      // 预览前确保有内容可渲染
      if (!this.llmEdit?.prompt) {
        try { this.llmEdit!.prompt = await invoke<string>("get_default_prompt"); } catch {}
      }
      this.editLlmPromptPreview = true;
    },
    async runLlmSelftest(this: AppThis) {
      if (!this.cfg || this.selftesting) return;
      this.selftesting = true;
      this.selftestResult = "";
      try {
        const r = await invoke<any>("llm_selftest", {
          profile: this.llmEdit ? {
            provider: this.llmEdit.provider,
            base_url: this.llmEdit.base_url,
            model: this.llmEdit.model,
            api_key: this.llmEdit.api_key,
            prompt: this.llmEdit.prompt,
            thinking: this.llmEdit.thinking,
          } : null,
        });
        this.selftestResult = [
          `① 词典纠错: ${r.dict_text}`,
          `② LLM(${r.tool_used ? "tool_calls ✓" : "⚠ 未走工具"}) ${r.latency_ms}ms: ${r.llm_text}`,
        ].join("\n");
      } catch (e) {
        this.selftestResult = `✗ 失败: ${String(e)}`;
      }
      this.selftesting = false;
    },
    async fetchLlmModels(this: AppThis) {
      try {
        this.llmModels = await invoke<string[]>("list_llm_models", {
          provider: this.llmEdit?.provider ?? "deepseek",
          baseUrl: this.llmEdit?.base_url ?? "",
          apiKey: this.llmEdit?.api_key ?? "",
        });
        this.llmModelsError = "";
        // 默认模型: 列表首个(当前值不在列表时)
        if (this.llmModels.length && !this.llmModels.includes(this.llmEdit?.model ?? "")) {
          this.llmEdit!.model = this.llmModels[0];
        }
      } catch (e) {
        this.llmModelsError = String(e);
        this.llmModels = [];
      }
    },
    // 思考强度选项: 每家来自各自官网 enum(现在恰好相同, 分叉时各自改)
    effortOptions(this: AppThis): string[] {
      return this.llmEdit?.provider === "zhipu"
        ? ["max", "high", "low"] // 智谱官网 enum: max/xhigh/high/medium/low/minimal/none, 5.3 系仅后三档
        : ["max", "high", "low"]; // DeepSeek 官网 enum: low/high/max
    },
    onLlmProviderChange(this: AppThis) {
      const defaults: Record<string, string> = {
        deepseek: "https://api.deepseek.com",
        zhipu: "https://open.bigmodel.cn/api/paas/v4",
      };
      if (this.llmEdit) {
        this.llmEdit.base_url = defaults[this.llmEdit.provider] ?? "";
        // effort 选项随供应商变化, 值重置为该家官网默认(智谱 max / DeepSeek high)
        const effDefault: Record<string, string> = { zhipu: "max", deepseek: "high" };
        this.llmEdit.effort = effDefault[this.llmEdit.provider] ?? "low";
        this.fetchLlmModels();
      }
    },

    // ── LLM 档案 ──
    newLlm(this: AppThis) {
      this.llmIsNew = true;
      this.llmEdit = { id: uid(), name: "新模型", provider: "deepseek", base_url: "", model: "", api_key: "", prompt: "", thinking: false, effort: "high" };
      this.editLlmPromptPreview = false;
      this.fetchLlmModels();
    },
    async editLlm(this: AppThis, p: LlmProfile) {
      this.llmIsNew = false;
      this.llmEdit = JSON.parse(JSON.stringify(p));
      // prompt 空则回填内置默认(直接显示真实将发送的内容)
      if (!this.llmEdit.prompt) {
        try { this.llmEdit.prompt = await invoke<string>("get_default_prompt"); } catch {}
      }
      this.fetchLlmModels();
    },
    async delLlm(this: AppThis, p: LlmProfile) {
      if (!this.cfg) return;
      this.cfg.llm_profiles = this.cfg.llm_profiles.filter((x) => x.id !== p.id);
      if (this.cfg.active_llm_id === p.id) this.cfg.active_llm_id = this.cfg.llm_profiles[0]?.id ?? "";
      await this.saveCfg();
    },
    async useLlm(this: AppThis, p: LlmProfile) {
      if (!this.cfg) return;
      this.cfg.active_llm_id = p.id;
      await this.saveCfg();
    },
    async saveLlm(this: AppThis) {
      if (!this.cfg || !this.llmEdit) return;
      if (this.llmEdit.api_key.trim() && !this.cfg.keys.includes(this.llmEdit.api_key.trim())) {
        this.cfg.keys.push(this.llmEdit.api_key.trim());
      }
      const i = this.cfg.llm_profiles.findIndex((x) => x.id === this.llmEdit!.id);
      if (i >= 0) this.cfg.llm_profiles[i] = this.llmEdit;
      else this.cfg.llm_profiles.push(this.llmEdit);
      if (!this.cfg.active_llm_id) this.cfg.active_llm_id = this.llmEdit.id;
      this.llmEdit = null;
      await this.saveCfg();
    },
    async togglePolish(this: AppThis) {
      if (!this.cfg) return;
      this.cfg.active_llm_id = this.cfg.active_llm_id ? "" : (this.cfg.llm_profiles[0]?.id ?? "");
      await this.saveCfg();
    },
  };
}
