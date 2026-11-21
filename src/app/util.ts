// 主窗公共工具(阶段A 第5步从 main.ts 拆出)
export const uid = () => Math.random().toString(36).slice(2, 10);

/// C54: 组合字符串美化("ctrl+cmd+space" → "⌃⌘ Space")
export function describeCombo(s: string): string {
  const tokens = comboTokens(s);
  if (!tokens.length) return "";
  const isMod = (t: string) => /^[⌃⌥⇧⌘]$/.test(t) || t === "Fn";
  const mods = tokens.filter(isMod);
  const rest = tokens.filter((t) => !isMod(t));
  if (!rest.length) return mods.join(" ");
  return (mods.includes("Fn") ? "Fn " : "") + mods.filter((m) => m !== "Fn").join("") + (mods.some((m) => m !== "Fn") ? " " : "") + rest.join(" ");
}

/// 拆分组合键为物理键帽数组("ctrl+cmd+space" → ["⌃", "⌘", "Space"])，供 <kbd> 渲染
export function comboTokens(s: string): string[] {
  const sym: Record<string, string> = { ctrl: "⌃", option: "⌥", alt: "⌥", shift: "⇧", cmd: "⌘", fn: "Fn" };
  const keyNames: Record<string, string> = {
    space: "Space", enter: "Return", backspace: "Delete", tab: "Tab", esc: "Esc",
    left: "←", right: "→", up: "↑", down: "↓", "`": "`", "-": "-", "=": "=",
    "[": "[", "]": "]", "\\": "\\", ";": ";", "'": "'", ",": ",", ".": ".", "/": "/",
  };
  const tokens = s.split("+").filter(Boolean).map((p) => {
    const lower = p.toLowerCase();
    if (sym[lower]) return sym[lower];
    if (keyNames[lower]) return keyNames[lower];
    if (/^f\d{1,2}$/.test(lower)) return lower.toUpperCase();
    return p.length <= 1 ? p.toUpperCase() : p.charAt(0).toUpperCase() + p.slice(1);
  });
  const isMod = (t: string) => /^[⌃⌥⇧⌘]$/.test(t) || t === "Fn";
  return [...tokens.filter(isMod), ...tokens.filter((t) => !isMod(t))];
}
