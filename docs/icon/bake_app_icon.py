"""把干净字形(icon3.svg)烘焙成 app 图标与状态栏图标。

用法: uv run --with numpy --with pillow --no-project python3 bake_app_icon.py [light|blue]

背景(实测):
- qlmanage 不渲染 SVG feDropShadow, 且输出 PNG 带不透明白底 → 手工 knock_out_white
- macOS 26 会给所有 app 图标加一层浅灰底板(macOS 自带), 因此卡片必须与浅灰底板有明显色差,
  否则渲染出来只剩一圈灰边(用户指出的问题)。闪电说=橙色卡片, Handy=粉色卡片。
- 字形留白: 设计稿字形宽/卡片=0.86, 之前只有 0.76 → 放大并居中。
"""
import subprocess
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

MODE = sys.argv[1] if len(sys.argv) > 1 else "light"

S = 1024
# 卡片几何: 复制 Handy(实测卡片 823/1024=80.4%, 圆角≈0.25*跨度, 纯色, 轻阴影)
X0 = Y0 = 100
X1 = Y1 = 923
SPAN = X1 - X0                      # 823
RAD = round(SPAN * 0.25)            # 206
GLYPH_W_RATIO = 0.86                # 字形宽 / 卡片宽(取自设计稿实测)
SHADOW_BLUR = 22
SHADOW_OFFSET = 6
SHADOW_ALPHA = 0.20

# 0) 渲染干净字形(icon3.svg 本身无背景无滤镜)
subprocess.run(["qlmanage", "-t", "-s", "1024", "-o", ".", "icon3.svg"], check=True, capture_output=True)
_raw = Image.open("icon3.svg.png").convert("RGBA")


def knock_out_white(im):
    """qlmanage 输出带不透明白底: 近白像素敲成透明; 白色 halo 一并透出。"""
    gray = im.convert("L")
    src_alpha = im.split()[3]
    out_alpha = Image.new("L", im.size, 0)
    la, ga, oa = src_alpha.load(), gray.load(), out_alpha.load()
    for y in range(im.height):
        for x in range(im.width):
            l = ga[x, y]
            if l > 215:
                oa[x, y] = 0
            elif l < 90:
                oa[x, y] = la[x, y]
            else:
                oa[x, y] = int(la[x, y] * (215 - l) / 125.0)
    out = im.copy()
    out.putalpha(out_alpha)
    return out


glyph = knock_out_white(_raw)
glyph.save("icon3_knockout.png")

# 1) 字形放大到目标比例并居中到卡片中心
gb = glyph.getbbox()
gw0, gh0 = gb[2] - gb[0], gb[3] - gb[1]
scale = (GLYPH_W_RATIO * SPAN) / float(gw0)
gnew = glyph.resize((round(glyph.width * scale), round(glyph.height * scale)), Image.LANCZOS)
gcx, gcy = (gb[0] + gb[2]) / 2.0 * scale, (gb[1] + gb[3]) / 2.0 * scale
ccx, ccy = (X0 + X1) / 2.0, (Y0 + Y1) / 2.0
goff = (round(ccx - gcx), round(ccy - gcy))

# 2) 卡片: 圆角矩形 + 纯色(Handy 风格; light 版保留渐变)
if MODE in ("light", "bw", "gray"):
    _glyph_dark = True
    _yy = np.arange(S, dtype=np.float64)[:, None]
    _tt = np.clip((_yy - Y0) / float(SPAN), 0.0, 1.0)
    if MODE == "gray":
        _solid = (234, 234, 237)                     # 贴近 macOS 26 系统底板色
        card = Image.new("RGB", (S, S), _solid)
    elif MODE == "bw":
        card = Image.new("RGB", (S, S), (255, 255, 255))
    else:
        _grad = np.array([255.0, 255.0, 255.0])[None, None, :] * (1.0 - _tt[..., None]) \
            + np.array([227.0, 231.0, 238.0])[None, None, :] * _tt[..., None]
        _grad = np.broadcast_to(_grad, (S, S, 3)).copy()
        _grad += np.random.default_rng(7).uniform(-0.7, 0.7, (S, S, 1))
        card = Image.fromarray(np.clip(_grad, 0, 255).astype(np.uint8), "RGB")
else:
    _glyph_dark = False
    _solid = (27, 123, 255) if MODE == "blue" else (242, 130, 178) if MODE == "handy" else (0, 0, 0)
    card = Image.new("RGB", (S, S), _solid)

# 遮罩: 4x 超采样后降采样 → 圆角边缘抗锯齿
_M = 4
_m4 = Image.new("L", (S * _M, S * _M), 0)
ImageDraw.Draw(_m4).rounded_rectangle([X0 * _M, Y0 * _M, X1 * _M, Y1 * _M], radius=RAD * _M, fill=255)
mask = _m4.resize((S, S), Image.LANCZOS)

canvas = Image.new("RGBA", (S, S), (231, 235, 241, 0))
if not _glyph_dark:
    # 轻阴影(复制 Handy: 卡片形状模糊+下移)
    _sa = mask.filter(ImageFilter.GaussianBlur(SHADOW_BLUR)).point(lambda v: int(v * SHADOW_ALPHA))
    _sh = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    _sh.putalpha(_sa)
    canvas.alpha_composite(_sh, (0, SHADOW_OFFSET))
card_rgba = card.convert("RGBA")
card_rgba.putalpha(mask)
canvas.alpha_composite(card_rgba, (0, 0))

# 3) 字形叠上去(浅色卡片 → 纯黑字形; 深色卡片 → 纯白字形, 保留原 alpha 作为形状)
if _glyph_dark:
    _g = Image.new("RGBA", gnew.size, (0, 0, 0, 255))
    _g.putalpha(gnew.split()[3])
else:
    _g = Image.new("RGBA", gnew.size, (255, 255, 255, 255))
    _g.putalpha(gnew.split()[3])
canvas.alpha_composite(_g, goff)

# 4) 全透明像素 RGB 统一: 避免缩放/预乘渲染时混入黑边
_px = canvas.load()
for y in range(S):
    for x in range(S):
        if _px[x, y][3] == 0:
            _px[x, y] = (231, 235, 241, 0)

name = {"light": "icon-app-1024.png", "blue": "icon-app-blue-1024.png", "handy": "icon-app-handy-1024.png",
        "bw": "icon-app-bw-1024.png", "gray": "icon-app-gray-1024.png", "black": "icon-app-black-1024.png"}[MODE]
canvas.save(name)
print("%s saved (card %d..%d, glyph scale %.3f, offset %s)" % (name, X0, X1, scale, goff))

# 5) 状态栏图标: 始终用原字形(黑 template)
bbox = glyph.getbbox()
gc = glyph.crop(bbox)
nw = round(gc.width * 32.0 / gc.height)
gm = gc.resize((nw, 32), Image.LANCZOS)
alpha = gm.split()[3]


def to_black_alpha(a):
    black = Image.new("RGBA", gm.size, (0, 0, 0, 255))
    black.putalpha(a)
    return black


if MODE == "light":
    to_black_alpha(alpha).save("menubar.png")
    to_black_alpha(alpha.filter(ImageFilter.MaxFilter(3))).save("menubar-rec.png")
    print("menubar.png / menubar-rec.png saved (size %dx%d)" % gm.size)
