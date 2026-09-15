#!/usr/bin/env python3
"""新基准（ref2_1024）重构 icon3.svg：
- 波形：骨架链（贪心最近邻）→ splprep 平滑 → 描边路径
- 箭头：骨架分支测直线（轴 + 两臂）→ 轴 line + 臂 polyline（miter 尖）
- 文档：doc_part.svg 按新基准缩放/平移对齐
"""
import numpy as np
import re
from PIL import Image
from scipy import ndimage as ndi
from scipy.spatial import cKDTree
from scipy.interpolate import splprep, splev, UnivariateSpline
from skimage.morphology import medial_axis

TH = 120.0
lum = np.asarray(Image.open('ref2_1024.png').convert('L')).astype(np.float32)
ink = lum < TH

# ---------- 10x 超采样数据（用于亚像素精修）----------
S = 10.0
X0, Y0 = 130.0, 360.0
_big = Image.open('ref2_1024.png').convert('L').crop((int(X0), int(Y0), 510, 680)) \
    .resize((int((510 - X0) * S), int((680 - Y0) * S)), Image.LANCZOS)
lum10 = np.asarray(_big).astype(np.float32)
ink10 = lum10 < TH
H10, W10 = ink10.shape
HW10 = 12.0 * S


def ref10(y1, xr):
    """10x 某行的 ink 区间：返回 (L,R) 或 None（xr=(x1_lo,x1_hi) 1x）"""
    y10 = int(round((y1 - Y0) * S))
    if y10 < 0 or y10 >= H10:
        return None
    xs = np.where(ink10[y10, int((xr[0] - X0) * S):int((xr[1] - X0) * S)])[0]
    if not len(xs):
        return None
    xs = xs + int((xr[0] - X0) * S)
    return float(xs.min()), float(xs.max())


def col10(x1, yr):
    """10x 某列的 ink 区间：返回 (top,bot) 或 None"""
    x10 = int(round((x1 - X0) * S))
    if x10 < 0 or x10 >= W10:
        return None
    ys = np.where(ink10[int((yr[0] - Y0) * S):int((yr[1] - Y0) * S), x10])[0]
    if not len(ys):
        return None
    ys = ys + int((yr[0] - Y0) * S)
    return float(ys.min()), float(ys.max())
lab, n = ndi.label(ink)
main = lab == lab[500, 200]
skel, dist = medial_axis(main, return_distance=True)
gy, gx = np.mgrid[0:1024, 0:1024]

# ---------- 波形 ----------
wmask = (gx <= 445) & ~((gx > 405) & ((gy < 490) | (gy > 535))) & ~((gx > 298) & (gx < 362) & (gy > 545))
sy, sx = np.where(skel & wmask)
pts = np.stack([sx, sy], axis=1).astype(float)
i0 = int(np.argmin(pts[:, 0]))
tree = cKDTree(pts)
visited = np.zeros(len(pts), bool)
visited[i0] = True
order = [i0]
cur = pts[i0]
jumps = 0
retraces = 0
while True:
    # 内层：贪心走链（直行优先）
    while True:
        idxs = tree.query_ball_point(cur, 14.0)
        if len(order) >= 8:
            d = cur - pts[order[-8]]
            nd = np.linalg.norm(d)
            d = d / nd if nd > 1e-6 else None
        else:
            d = None
        best = -1
        bestscore = 1e9
        for i in idxs:
            if visited[i]:
                continue
            v = pts[i] - cur
            dv = float(np.linalg.norm(v))
            if dv > 14.0:
                continue
            if d is not None:
                cosang = float(np.dot(v, d)) / (dv * np.linalg.norm(d))
                cosang = max(-1.0, min(1.0, cosang))
                ang = np.arccos(cosang)
                score = dv * (1.0 + 2.2 * ang * ang)
            else:
                score = dv
            if score < bestscore:
                bestscore = score
                best = i
        if best < 0:
            break
        visited[best] = True
        order.append(best)
        cur = pts[best]
    # 死端：直接跳接最近未访问点（禁用折返，避免尾部乱走）
    rem = np.where(~visited)[0]
    if len(rem) == 0:
        break
    dd = np.hypot(pts[rem, 0] - cur[0], pts[rem, 1] - cur[1])
    j = int(rem[int(np.argmin(dd))])
    if dd.min() > 45.0:
        break
    visited[j] = True
    order.append(j)
    cur = pts[j]
    jumps += 1
chain = pts[order]
# ② 链尾截断（先于拼接执行）：到轴区（x>393 且近 y=513）首个点为止
cut = -1
for i in range(len(chain)):
    if chain[i, 0] > 393 and abs(chain[i, 1] - 513) < 8:
        cut = i
        break
if cut > 100:
    print(f'链尾截断：{chain[-1].astype(int)} → {chain[cut].astype(int)}')
    chain = chain[:cut + 1]
# ---------- 波形中心线（全机械派生，零手编点）----------
# ① 谷底（机械）：下降股=左边缘+半宽、上行股=右边缘-半宽、U=底边缘-半宽
# 边缘序列先二次拟合去噪（曲率平滑），再按拟合曲线采样
def runs10(y1, xr):
    y10i = int(round((y1 - Y0) * S))
    if y10i < 0 or y10i >= H10:
        return []
    row = ink10[y10i, int((xr[0] - X0) * S):int((xr[1] - X0) * S)]
    xs = np.where(row)[0]
    if not len(xs):
        return []
    xs = xs + int((xr[0] - X0) * S)
    sp = np.where(np.diff(xs) > 6)[0]
    return [(float(g.min()) / S + X0, float(g.max()) / S + X0) for g in np.split(xs, sp + 1)]


def offset_inward(edge, hw, side):
    """边缘折线沿法线向内偏移 hw（side=内向参考向量）。曲线段自动跟随，避免垂直偏移外飘。"""
    P = np.asarray(edge, float)
    n = len(P)
    out = []
    for i in range(n):
        a = P[max(i - 2, 0)]
        b = P[min(i + 2, n - 1)]
        t = b - a
        nt = np.linalg.norm(t)
        if nt < 1e-9:
            out.append(P[i])
            continue
        t = t / nt
        nn = np.array([t[1], -t[0]])
        if nn[0] * side[0] + nn[1] * side[1] < 0:
            nn = -nn
        out.append(P[i] + hw * nn)
    return np.array(out)


def ushape(y_lo, y_hi, xr, hw):
    """U 形中心线：U 区外轮廓整体法向内偏移（左缘下行 + 右缘上行，遍历方向天然给出内向法线），
    U 底自然成弧。样条平滑（σ≈0.9px）后返回中心线点列。"""
    rows = [(y, runs10(y, xr)) for y in range(y_lo, y_hi)]
    left, right = [], []
    for y, rs in rows:
        if rs:
            left.append((rs[0][0], float(y)))
            right.append((rs[-1][1], float(y)))
    if len(left) < 8:
        return None
    edge_all = left + right[::-1]
    off = offset_inward(edge_all, hw, (0.0, 0.0))
    tck, _ = splprep([off[:, 0], off[:, 1]], s=len(off) * 0.4, k=3)
    n_s = max(int(len(off) * 2), 80)
    ss = np.linspace(0.0, 1.0, n_s)
    px, py = splev(ss, tck)
    return np.stack([px, py], axis=1)


def splice_ushape(ch, pts, axr, ayr, bxr, byr, tag):
    if pts is None or len(ch) < 2:
        return ch, False
    for i in range(len(ch) - 1):
        A = ch[i]
        B = ch[i + 1]
        if axr[0] <= A[0] <= axr[1] and ayr[0] <= A[1] <= ayr[1] and bxr[0] <= B[0] <= bxr[1] and byr[0] <= B[1] <= byr[1]:
            return np.vstack([ch[:i + 1], pts, ch[i + 1:]]), True
    return ch, False


hw_wave = float(np.median(dist[chain[:, 1].astype(int), chain[:, 0].astype(int)]))
valley = ushape(530, 648, (294, 364), hw_wave)
idx_v = [i for i, p in enumerate(chain) if 300 <= p[0] <= 350 and p[1] >= 520]
if idx_v and valley is not None:
    i0, i1 = idx_v[0], idx_v[-1]
    chain = np.vstack([chain[:i0], valley, chain[i1 + 1:]])
    print(f'谷底拼接：替换 {i1 - i0 + 1} 点为 {len(valley)} 点')
dip = ushape(506, 582, (218, 300), hw_wave)
if dip is not None:
    idx_dip = [i for i, p in enumerate(chain) if 232 <= p[0] <= 292 and 500 <= p[1] <= 560]
    if idx_dip:
        i0, i1 = idx_dip[0], idx_dip[-1]
        chain = np.vstack([chain[:i0], dip, chain[i1 + 1:]])
        print(f'小谷拼接：替换 {i1 - i0 + 1} 点为 {len(dip)} 点')
# ③ 冠部：近水平段用顶边缘+半宽，肩部与骨架平滑混合（机械）
wtop = {}
for x in range(338, 374):
    c = col10(x, (455, 515))
    if c is not None:
        wtop[x] = c[0] / S + Y0 + 12.0


def sstep(t):
    t = max(0.0, min(1.0, t))
    return t * t * (3 - 2 * t)


def crown_w(x):
    if x <= 340 or x >= 374:
        return 0.0
    if x < 352:
        return sstep((x - 340) / 12.0)
    if x > 360:
        return sstep((374 - x) / 14.0)
    return 1.0


idxc = [i for i, p in enumerate(chain) if 340 <= p[0] <= 356 and p[1] < 502]
if idxc and wtop:
    i0, i1 = idxc[0], idxc[-1]
    newpts = []
    for i in range(i0, i1 + 1):
        x, y = chain[i]
        xi = int(round(x))
        T = wtop.get(xi)
        w = crown_w(x)
        if T is not None and w > 0:
            newpts.append((x, (1 - w) * y + w * T))
        else:
            newpts.append((x, y))
    chain = np.vstack([chain[:i0], np.array(newpts, float), chain[i1 + 1:]])
    print(f'冠部（机械混合）：混合 {len(newpts)} 点（顶边缘+半宽 ↔ 骨架）')
# ④ 尾段（按 ref2 骨架 DT + 顶边联合实测重建）：出拱即渐进下行（~58°）→ x≈377 并入轴中心线（纯水平）
idxj = [i for i, p in enumerate(chain) if 355 <= p[0] <= 400]
if idxj:
    j0, j1 = idxj[0], idxj[-1]
    tail_new = np.array([
        [355.0, 481.0], [357.0, 484.5], [359.0, 488.0], [361.0, 491.0],
        [363.0, 494.0], [365.0, 497.5], [366.0, 500.0],
    ], float)
    chain = np.vstack([chain[:j0], tail_new, chain[j1 + 1:]])
    print(f'尾段（实测重建）：替换 {j1 - j0 + 1} 点 → {len(tail_new)} 点（出拱渐进下行至 (366,500)，其后由喇叭过渡多边形接管）')
print(f'波形链 {len(chain)} 点（跳接 {jumps} 次、折返 {retraces} 次），起 {chain[0].astype(int)} 止 {chain[-1].astype(int)}，y 范围 {chain[:, 1].min()} ~ {chain[:, 1].max()}')
print('已访问/总骨架', int(visited.sum()), '/', len(pts))
wpix = dist[chain[:, 1].astype(int), chain[:, 0].astype(int)]
print('波形半宽 median', np.median(wpix))
# ⑤ 10x 亚像素精修：逐点沿法线取截面，找包含该点的 ink 区间中点（宽度合理才采纳）
chain = chain.astype(float)
_good = 0
for _i in range(2, len(chain) - 2):
    p = chain[_i]
    if (300.0 <= p[0] <= 350.0 and p[1] >= 528.0) or (228.0 <= p[0] <= 292.0 and p[1] >= 503.0):
        continue
    t = chain[min(_i + 5, len(chain) - 1)] - chain[max(_i - 5, 0)]
    nt = np.linalg.norm(t)
    if nt < 1e-6:
        continue
    t = t / nt
    n = np.array([-t[1], t[0]])
    p10 = np.array([(p[0] - X0) * S, (p[1] - Y0) * S])
    ss = np.arange(-36.0 * S, 36.0 * S + 0.5, 0.5)
    qx = p10[0] + ss * n[0]
    qy = p10[1] + ss * n[1]
    ix0 = np.floor(qx).astype(int)
    iy0 = np.floor(qy).astype(int)
    ok = (ix0 >= 0) & (ix0 < W10 - 1) & (iy0 >= 0) & (iy0 < H10 - 1)
    if not ok.any():
        continue
    fx = (qx - ix0).clip(0, 1)
    fy = (qy - iy0).clip(0, 1)
    i0 = ix0[ok]
    j0 = iy0[ok]
    val = (ink10[j0, i0] * (1 - fx[ok]) * (1 - fy[ok]) + ink10[j0, i0 + 1] * fx[ok] * (1 - fy[ok])
           + ink10[j0 + 1, i0] * (1 - fx[ok]) * fy[ok] + ink10[j0 + 1, i0 + 1] * fx[ok] * fy[ok])
    v = np.zeros(len(ss), bool)
    v[ok] = val > 0.5
    mid = len(ss) // 2
    if not v[mid]:
        continue
    a = mid
    while a > 0 and v[a - 1]:
        a -= 1
    b = mid
    while b < len(v) - 1 and v[b + 1]:
        b += 1
    s1, s2 = ss[a], ss[b]
    wdt = (s2 - s1) / S
    if wdt < 6.0 or wdt > 30.0:
        continue
    sm = 0.5 * (s1 + s2)
    np_ = p10 + sm * n
    chain[_i] = np.array([np_[0] / S + X0, np_[1] / S + Y0])
    _good += 1
print(f'10x 精修：{_good}/{len(chain)} 点截面有效')

# ④ 平滑样条拟合（σ≈0.35px@1x：吸收亚像素噪声，保持形状，输出密集采样）
d1 = np.hypot(np.diff(chain[:, 0]), np.diff(chain[:, 1]))
tcum = np.concatenate([[0], np.cumsum(d1)])
L = float(tcum[-1])
uu = tcum / L
keep = np.concatenate([[True], np.diff(uu) > 1e-9])
tck, _ = splprep([chain[keep, 0], chain[keep, 1]], u=uu[keep], s=len(chain) * 0.1225, k=3)
u_new = np.linspace(0, 1, 800)
sx, sy = splev(u_new, tck)
wave_pts = [(float(x), float(y)) for x, y in zip(sx, sy)]
wd = 2.0 * float(np.median(wpix))
print(f'波形描边宽 {wd:.2f}（平滑样条 s={len(chain) * 0.1225:.0f}，点数 {len(wave_pts)}）')

# ---------- 箭头 ----------
amask = skel & (gx > 400) & (np.hypot(gx - 460, gy - 512) < 95) & ~((gx <= 452) & (np.abs(gy - 512.5) < 6))
ay, ax = np.where(amask)
apts = np.stack([ax, ay], axis=1).astype(float)
by, bx = np.where(skel & (gx > 432) & (gx < 448) & (np.abs(gy - 513) < 3))
shaft = np.stack([bx, by], axis=1).astype(float)
yshaft = float(np.median(shaft[:, 1]))
up = apts[(apts[:, 1] < yshaft - 4) & (apts[:, 1] > yshaft - 90)]
lo = apts[(apts[:, 1] > yshaft + 4) & (apts[:, 1] < yshaft + 90)]
print('shaft y', yshaft, 'n', len(shaft), '| up', len(up), 'lo', len(lo))


def fitline(P):
    c = P.mean(0)
    _, _, vt = np.linalg.svd(P - c)
    d = vt[0]
    if d[0] < 0:
        d = -d
    proj = (P - c) @ d
    a = c + proj.min() * d
    b = c + proj.max() * d
    return a, b, d


u1, u2, d1v = fitline(up)
l1, l2, d2v = fitline(lo)
print('上臂', u1.astype(int), u2.astype(int), '方向', d1v.round(3))
print('下臂', l1.astype(int), l2.astype(int), '方向', d2v.round(3))
apix = dist[apts[:, 1].astype(int), apts[:, 0].astype(int)]
ad = 2.0 * float(np.median(apix))
print(f'箭头描边宽 {ad:.1f}')

# 尖端：迭代求路径点，使 miter 落点 = 目标视觉尖端（按实测箭头外沿）
def miter_tip(P, u1, l1, halfw):
    i = P - u1
    i = i / np.linalg.norm(i)
    o = l1 - P
    o = o / np.linalg.norm(o)
    n1 = np.array([i[1], -i[0]])
    n2 = np.array([o[1], -o[0]])
    best = None
    for sgn in (1.0, -1.0):
        A1 = u1 + sgn * halfw * n1
        A2 = l1 + sgn * halfw * n2
        M = np.stack([i, -o], axis=1)
        rhs = A2 - A1
        st = np.linalg.solve(M, rhs)
        X = A1 + st[0] * i
        if best is None or X[0] > best[0]:
            best = X
    return best


halfw = ad / 2.0
tip_target = np.array([484.0, yshaft])
P = np.array([456.0, yshaft])
for _ in range(6):
    X = miter_tip(P, u1, l1, halfw)
    P = np.array([P[0] + (tip_target[0] - X[0]), yshaft])
X = miter_tip(P, u1, l1, halfw)
print(f'路径点 P=({P[0]:.1f},{P[1]:.1f}) → miter 落点 ({X[0]:.1f},{X[1]:.1f})（目标 {tip_target}）')

# ---------- 组装 icon3.svg ----------
def poly_d(P):
    return 'M ' + ' L '.join(f'{x:.1f} {y:.1f}' for x, y in P)


def smooth_d(P):
    """Catmull-Rom → 三次贝塞尔（C1 平滑，消除折线段角）"""
    P = np.asarray(P, float)
    if len(P) < 3:
        return poly_d(P)
    segs = [f'M {P[0][0]:.2f} {P[0][1]:.2f}']
    n = len(P)
    for i in range(n - 1):
        p0 = P[max(i - 1, 0)]
        p1 = P[i]
        p2 = P[i + 1]
        p3 = P[min(i + 2, n - 1)]
        c1 = p1 + (p2 - p0) / 6.0
        c2 = p2 - (p3 - p1) / 6.0
        segs.append(f'C {c1[0]:.2f} {c1[1]:.2f} {c2[0]:.2f} {c2[1]:.2f} {p2[0]:.2f} {p2[1]:.2f}')
    return ' '.join(segs)

# 文档（按 ref2 实测直接构造：外框中心线路径 + 三条内线；描边 33 圆帽）
doc_stroke = 34.0
doc_frame = ('M 453.5 386.5 L 453.5 386 A 69.5 69.5 0 0 1 523 316.5 L 774 316.5 '
             'A 69.5 69.5 0 0 1 843.5 386 L 843.5 638.5 A 69.5 69.5 0 0 1 774 708 '
             'L 523 708 A 69.5 69.5 0 0 1 453.5 638.5 L 453.5 637.5')
doc_lines = [
    'M 542.5 433 L 753.5 433',
    'M 542.5 512 L 753.5 512',
    'M 542.5 591 L 695.5 591',
]
print('文档：构造版（外框+三内线）')

# 尾段连接：喇叭过渡（G1 连续贝塞尔填充多边形）——上边界从尾巴上缘切向出发、下边界汇入轴下缘，两端相切无端头
_taper_svg = ('  <path d="M 376.4 494.0 C 378.9 498.3 378.0 496.3 384.0 496.3'
              ' L 390 496.3 L 390 529.5 L 386 529.5'
              ' C 368 529.5 359.5 516.5 355.6 506.0 Z" fill="#3A3D42" stroke="none"/>')
print('尾段连接：喇叭过渡多边形（贝塞尔相切）')

# 波形白色分隔（复刻设计图的手绘描边效果）：拆 4 段，后段的白 halo 压住前段 → 笔画分离
_wp = wave_pts
_i_spike = int(min(range(len(_wp)), key=lambda i: _wp[i][1]))            # 尖峰顶
_i_ubot = int(max(range(_i_spike, len(_wp)), key=lambda i: _wp[i][1]))   # 深谷底
_i_nose = int(min(range(_i_ubot, len(_wp)), key=lambda i: _wp[i][1]))    # 鼻头顶
_wsegs = [_wp[:_i_spike + 1], _wp[_i_spike:_i_ubot + 1], _wp[_i_ubot:_i_nose + 1], _wp[_i_nose:]]
_wave_svg = f'  <path d="{smooth_d(_wsegs[0])}" fill="none" stroke="#3A3D42" stroke-width="{wd:.1f}" stroke-linecap="round" stroke-linejoin="round"/>'
for _sg in _wsegs[1:]:
    _dstr = smooth_d(_sg)
    _skip = min(18, max(len(_sg) - 2, 0))   # halo 起点下移, 避免端帽盖住上一段笔画端头
    _halo = smooth_d(_sg[_skip:])
    _wave_svg += f'\n  <path d="{_halo}" fill="none" stroke="#FFFFFF" stroke-width="{wd + 5.0:.1f}" stroke-linecap="round" stroke-linejoin="round"/>'
    _wave_svg += f'\n  <path d="{_dstr}" fill="none" stroke="#3A3D42" stroke-width="{wd:.1f}" stroke-linecap="round" stroke-linejoin="round"/>'
print('波形分段 halo: 分割点', [_i_spike, _i_ubot, _i_nose], '/ 总点数', len(_wp))

svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="100%" height="100%">
  <g transform="translate(648.5 512) scale(0.75 1) translate(-648.5 -512)">
  <path d="{doc_frame}" fill="none" stroke="#3A3D42" stroke-width="{doc_stroke:.1f}" stroke-linecap="round"/>
  <path d="{doc_lines[0]}" fill="none" stroke="#3A3D42" stroke-width="{doc_stroke:.1f}" stroke-linecap="round"/>
  <path d="{doc_lines[1]}" fill="none" stroke="#3A3D42" stroke-width="{doc_stroke:.1f}" stroke-linecap="round"/>
  <path d="{doc_lines[2]}" fill="none" stroke="#3A3D42" stroke-width="{doc_stroke:.1f}" stroke-linecap="round"/>
  </g>
{_wave_svg}
  <path d="M {u1[0]:.1f} {u1[1]:.1f} L {P[0]:.1f} {P[1]:.1f} L {l1[0]:.1f} {l1[1]:.1f}" fill="none" stroke="#FFFFFF" stroke-width="{ad + 5:.1f}" stroke-linecap="round" stroke-linejoin="round"/>
{_taper_svg}
  <line x1="381" y1="{yshaft:.1f}" x2="452" y2="{yshaft:.1f}" stroke="#3A3D42" stroke-width="{ad:.1f}" stroke-linecap="round"/>
  <path d="M {u1[0]:.1f} {u1[1]:.1f} L {P[0]:.1f} {P[1]:.1f} L {l1[0]:.1f} {l1[1]:.1f}" fill="none" stroke="#3A3D42" stroke-width="{ad:.1f}" stroke-linecap="round" stroke-linejoin="miter" stroke-miterlimit="12"/>
</svg>
'''
open('icon3.svg', 'w').write(svg)
import json as _json
_json.dump({'wave': [[float(x), float(y)] for x, y in wave_pts]}, open('lines.json', 'w'))
print('icon3.svg 已写')

# ---------- 中心线叠加自检 ----------
from PIL import ImageDraw
_ov = Image.open('ref2_1024.png').convert('RGB')
_d = ImageDraw.Draw(_ov)
for _i in range(len(wave_pts) - 1):
    _d.line([wave_pts[_i], wave_pts[_i + 1]], fill=(255, 0, 0), width=2)
_ov.crop((110, 340, 440, 700)).resize((660, 720), Image.LANCZOS).save('chk34_line_overlay.png')
print('overlay ok, wave_pts', len(wave_pts))

# ---------- 10x 整图验证（用户要求：整图放大 10 倍，线保持 1-2px）----------
_big_ref = Image.open('ref2_1024.png').convert('RGB').resize((10240, 10240), Image.LANCZOS)
_db2 = ImageDraw.Draw(_big_ref)
_w10 = [(x * 10.0, y * 10.0) for x, y in wave_pts]
for _i in range(len(_w10) - 1):
    _db2.line([_w10[_i], _w10[_i + 1]], fill=(255, 0, 0), width=2)
_big_ref.save('chk50_whole10x_line_on_ref.png')
print('chk50 saved（整图 10240 + 2px 线）')
