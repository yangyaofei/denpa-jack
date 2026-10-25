// 主窗公共工具(阶段A 第5步从 main.ts 拆出)
export const uid = () => Math.random().toString(36).slice(2, 10);

/// C54: 组合字符串美化("ctrl+cmd+space" → "⌃⌘ Space")
export function describeCombo(s: string): string {
  const sym: Record<string, string> = { ctrl: "⌃", option: "⌥", alt: "⌥", shift: "⇧", cmd: "⌘", fn: "Fn" };
  const keyNames: Record<string, string> = {
    space: "Space", enter: "Return", backspace: "Delete", tab: "Tab", esc: "Esc",
    left: "←", right: "→", up: "↑", down: "↓", "`": "`", "-": "-", "=": "=",
    "[": "[", "]": "]", "\\": "\\", ";": ";", "'": "'", ",": ",", ".": ".", "/": "/",
  };
  const parts = s.split("+").map((p) => {
    const lower = p.toLowerCase();
    if (sym[lower]) return sym[lower];
    if (keyNames[lower]) return keyNames[lower];
    if (/^f\d{1,2}$/.test(lower)) return lower.toUpperCase();
    return p.length <= 1 ? p.toUpperCase() : p.charAt(0).toUpperCase() + p.slice(1);
  });
  // 修饰符号直接连排, 与主键之间留空格
  const modSyms = parts.filter((p) => /^[⌃⌥⇧⌘]$/.test(p) || p === "Fn");
  const rest = parts.filter((p) => !modSyms.includes(p) || p === "Fn");
  const main = rest.filter((p) => p !== "Fn");
  if (!main.length) return modSyms.join(" ");
  return modSyms.join("") + " " + main.join(" ");
}
