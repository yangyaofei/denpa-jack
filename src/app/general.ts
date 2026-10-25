// 通用页域(阶段A 第5步从 main.ts 拆出): 热键绑定/触发模式/麦克风优先级/活动目标/粘贴名单文本域。
import { invoke } from "@tauri-apps/api/core";
import { describeCombo } from "./util";
import type { AppThis } from "./types";

export function generalPage(this: AppThis) {
  return {
    hotkeyHint: "",
    hkCapturing: false,
    hkCurrent: "",
    hkFinal: "",
    _capTimer: 0 as any,
    /** 粘贴优先名单的文本域(每行一个 bundle id; saveCfg 转数组, loadCfg 反向填充) */
    axPasteOnlyText: "" as string,

    // ── 快捷键录制 ──
    describeCombo,
    hotkeyLabel(this: AppThis) {
      // C54: 顶部文案取 bindings 列表第一个(迁移后单一真源)
      const first = this.cfg?.bindings?.["transcribe"]?.current?.[0];
      return first ? describeCombo(first) : "";
    },

    // C54: 绑定列表(bindings.transcribe.current 字符串数组, 单一真源)
    hkList(this: AppThis): string[] {
      return this.cfg?.bindings?.["transcribe"]?.current ?? [];
    },
    async removeHotkey(this: AppThis, combo: string) {
      try {
        await invoke("remove_binding", { combo });
        await this.loadCfg();
      } catch (x: any) { this.error = String(x); }
    },
    // C55 后端录制(用户定则): begin → 200ms 轮询 current/final → 展示结果 → 添加/取消
    // 引擎收全键(含 Fn/纯修饰/左右修饰); Esc 取消; 不会阻塞或卡住(end 无条件 resume)
    async startHotkeyRecord(this: AppThis) {
      if (this.hkCapturing) return;
      try { await invoke("capture_begin"); } catch (x: any) { this.error = String(x); return; }
      this.hkCapturing = true;
      this.hkCurrent = "";
      this.hkFinal = "";
      this.hotkeyHint = "";
      const escWatch = (e: KeyboardEvent) => {
        if (e.key === "Escape") {
          e.preventDefault();
          window.removeEventListener("keydown", escWatch, true);
          this.cancelHotkey();
        }
      };
      window.addEventListener("keydown", escWatch, true);
      this._capTimer = setInterval(async () => {
        try {
          const s = await invoke<any>("capture_poll");
          if (s.current) this.hkCurrent = s.current;
          if (s.final_combo) {
            this.hkFinal = s.final_combo;
            clearInterval(this._capTimer);
            window.removeEventListener("keydown", escWatch, true);
          } else if (s.error) {
            this.error = s.error;
            clearInterval(this._capTimer);
            window.removeEventListener("keydown", escWatch, true);
            this.hkCapturing = false; this.hkCurrent = "";
          }
        } catch { clearInterval(this._capTimer); this.hkCapturing = false; }
      }, 200);
    },
    async confirmHotkey(this: AppThis) {
      if (!this.hkFinal) return;
      try {
        await invoke("capture_end", { confirm: true, combo: this.hkFinal });
        this.hkCapturing = false; this.hkFinal = ""; this.hkCurrent = ""; this.hotkeyHint = "";
        await this.loadCfg();
      } catch (x: any) { this.error = String(x); this.hkCapturing = false; }
    },
    async cancelHotkey(this: AppThis) {
      clearInterval(this._capTimer);
      try { await invoke("capture_end", { confirm: false, combo: null }); } catch {}
      this.hkCapturing = false; this.hkCurrent = ""; this.hkFinal = ""; this.hotkeyHint = "";
    },
    async addFnPreset(this: AppThis, preset: string) {
      try {
        await invoke("add_binding", { combo: preset });
        await this.loadCfg();
      } catch (x: any) { this.error = String(x); }
    },

    async setActivation(this: AppThis, ev: Event) {
      if (!this.cfg) return;
      this.cfg.activation = (ev.target as HTMLSelectElement).value;
      await this.saveCfg();
    },
    async setMicDevice(this: AppThis, uid: string) {
      if (!this.cfg) return;
      this.cfg.mic_device_uid = uid;
      await this.saveCfg();
    },
    async addToPriority(this: AppThis) {
      if (!this.cfg || !this.micToAdd) return;
      if (!this.cfg.mic_priority.includes(this.micToAdd)) {
        this.cfg.mic_priority.push(this.micToAdd);
        await this.saveCfg();
      }
      this.micToAdd = "";
    },
    async movePriority(this: AppThis, i: number, dir: -1 | 1) {
      if (!this.cfg) return;
      const j = i + dir;
      if (j < 0 || j >= this.cfg.mic_priority.length) return;
      const arr = this.cfg.mic_priority;
      [arr[i], arr[j]] = [arr[j], arr[i]];
      await this.saveCfg();
    },
    async removePriority(this: AppThis, i: number) {
      if (!this.cfg) return;
      this.cfg.mic_priority.splice(i, 1);
      await this.saveCfg();
    },
    async switchMic(this: AppThis, ev: Event) {
      if (!this.cfg) return;
      this.cfg.mic_device_uid = (ev.target as HTMLSelectElement).value;
      await this.saveCfg();
      try { this.activeMic = await invoke<[string, string] | null>("get_active_mic"); } catch {}
    },

    // 活动目标: 保存后若不是"关闭", 主动申请屏幕录制权限并刷新权限列表(缺权限时卡片会给出提示)
    async onDetectTargetChange(this: AppThis) {
      await this.saveCfg();
      if (this.cfg.detect_active_target !== "off") {
        try { await invoke("request_screen_permission"); } catch (e) { console.warn("request_screen_permission 失败", e); }
      }
      await this.checkPerms();
    },

    async openScreenContextDir(this: AppThis) {
      try { await invoke("open_screen_context_dir"); } catch (e) { console.error(e); }
    },
  };
}
