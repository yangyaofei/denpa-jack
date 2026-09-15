# 应用图标（app icon + 状态栏图标）

本目录是图标的**唯一权威来源**：定稿资产 + 可复现的生成脚本。

## 定稿资产

| 文件 | 用途 |
|---|---|
| `icon.svg` | 字形母版（干净矢量：波形→箭头→文档，无卡片、无阴影） |
| `icon-app-1024.png` | app 图标 1024 源图（白卡片 + 纯黑字形） |
| `icon-app.icns` | app 图标 icns（十档 16…1024，供 `src-tauri/icons/` 打包） |
| `menubar.png` | 状态栏图标（字形裁剪→高 32@2x→纯黑 alpha，template 图） |
| `menubar-rec.png` | 状态栏录音态（`menubar.png` 加粗版，运行时染红） |
| `ref2_1024.png` | 参考基准图（由 `icon_draft.jpeg` 大图标规范化到 1024） |

装机位置：`src-tauri/icons/`（icon.icns / icon.png / 128x128.png / 128x128@2x.png / 32x32.png / menubar*.png）。

## 生成脚本（可复现）

```bash
cd docs/icon
# 1) 从参考图提取字形中心线 → 输出 icon.svg（纯字形，无卡片）
uv run --with numpy --with pillow --with scipy --with scikit-image python3 build5.py
# 2) 烘焙 app 图标与状态栏图标（bw = 白卡片 + 黑字形；另有 gray/black/light/blue/handy 模式）
uv run --with numpy --with pillow python3 bake_app_icon.py bw
```

- `build5.py`：骨架提取 + 10x 亚像素精修 + 样条平滑 → `icon.svg`。字形几何以参考图实测为准
  （谷底/小谷 U 形、冠部、与横轴汇入处均按实测重建）。
- `bake_app_icon.py`：`qlmanage` 渲染 `icon.svg` → 敲掉白底得字形 alpha → 与卡片合成 → 导出 1024 图与状态栏图。
- 验收口径（用户定则）：**必须看打包后 Finder 里的真实渲染**，不能只看本目录的 PNG——
  macOS 26 会给每个 app 图标叠一层系统底板，卡片颜色/大小需与之协调（当前定稿为白卡片 + 纯黑字形）。

## 设计决策记录（历史）

| 迭代 | 结论 |
|---|---|
| 手调曲线 | 边缘误差大（IoU 83%），放弃 |
| vtracer 像素追踪 + σ 平滑 | 指标高（IoU 97.3%）但有凸起毛刺，放弃 |
| 中心线 + 傅里叶 / 样条 | 最终路线：机械提取中心线 + 样条平滑（`build5.py`） |
| 卡片配色 | 近白卡片在 macOS 26 系统底板上只剩一圈灰边 → 逐一试过蓝/粉/纯黑卡片，最终定为**白卡片 + 纯黑字形** |
| 卡片结构 | 卡片 80.4% 画布、圆角 25% 跨度、纯色、轻阴影（对齐 Handy 的打包结构） |
| 字号留白 | 字形宽 = 卡片宽 86%（对齐设计稿比例） |
