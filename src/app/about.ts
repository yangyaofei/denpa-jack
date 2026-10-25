// 关于页域(阶段A 第5步从 main.ts 拆出): 应用内更新检查与安装。
import { invoke } from "@tauri-apps/api/core";
import type { AppThis } from "./types";

export function aboutPage(this: AppThis) {
  return {
    // 应用内更新状态(关于页): idle=未检查 / checking / latest=已是最新 / available=有新版本 / error
    updateState: "idle" as "idle" | "checking" | "latest" | "available" | "error",
    updateCheckedAt: "",
    updateInfo: null as any,
    updateBusy: false,
    updateProgress: "",
    updateErr: "",

    // ==== 应用内更新(关于页) ====
    // 状态文字要能区分"检查过、已是最新"和"从未检查"——否则用户看到"未检查"会以为功能没生效
    updateStateText(this: AppThis): string {
      if (this.updateState === "checking") return this.updateProgress || "检查中…";
      if (this.updateState === "error") return `失败：${this.updateErr}`;
      if (this.updateState === "available" && this.updateInfo)
        return `有新版本 ${this.updateInfo.version}（当前 ${this.updateInfo.current}）`;
      if (this.updateState === "latest")
        return `已是最新${this.updateCheckedAt ? `（最后检查 ${this.updateCheckedAt}）` : ""}`;
      return `当前 ${this.version || "—"}；未检查`;
    },
    // silent=true 用于启动时自动检查(失败不打扰、只记控制台)
    async checkUpdate(this: AppThis, silent = true) {
      if (this.updateBusy) return;
      this.updateBusy = true;
      this.updateErr = "";
      this.updateState = "checking";
      this.updateProgress = "检查中…";
      try {
        const info = await invoke<any | null>("update_check");
        this.updateInfo = info;
        this.updateState = info ? "available" : "latest";
        this.updateCheckedAt = new Date().toLocaleTimeString();
        this.updateProgress = "";
      } catch (e: any) {
        this.updateErr = String(e);
        this.updateInfo = null;
        this.updateState = "error";
        if (silent) console.warn("自动检查更新失败（已忽略）", e);
      } finally {
        this.updateBusy = false;
      }
    },
    async installUpdate(this: AppThis) {
      if (this.updateBusy) return;
      this.updateBusy = true;
      this.updateErr = "";
      this.updateProgress = "下载中…";
      try {
        await invoke("update_install"); // 成功后应用会重启，不会返回
      } catch (e: any) {
        this.updateErr = String(e);
        this.updateProgress = "";
        this.updateState = "error";
        this.updateBusy = false;
      }
    },
  };
}
