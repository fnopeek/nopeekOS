// The per-pixel paint of each scene item (compose.rs), shared by every
// program that paints them: compose.comp, and on V3D the fragment shader
// that draws one item (gpu_bcm2712/shaders/draw.frag). A bit-exact port of
// the kernel's CPU executor (kernel/src/shade/scene.rs `paint` and the
// helpers it calls in gui/render.rs, gui/background.rs and gui/theme.rs);
// comments name the function each part mirrors.
//
// The includer declares `Words` (a buffer reference to 32-bit words), `Blk`
// (an item's 16 words) and `BC(b, k)` (word k of one), defines PANEL_PLANES,
// PANEL_BSW, PANEL_BSH, PANEL_BX0 and PANEL_BY0 (the blurred planes a
// panel's base is read from, 0 wide for none, and the screen pixel they
// start at in quarters), and sets the g_* frame values before painting.

Words mkaddr(uint lo, uint hi) {
    return Words(uint64_t(lo) | (uint64_t(hi) << 32));
}

// The frame block, uniform per dispatch.
uint g_flags;
uint g_ceil;
uint g_tint;
uint g_tint_w;
Words g_wall;
uint g_wall_pitch;
Words g_blur;
uint g_blur_pitch;
Words g_glass;
uint g_glass_pitch;
Words g_glyphs;

// render::blend, alpha 0..=256.
uint blend(uint fg, uint bg, uint a) {
    uint inv = 256u - a;
    uint r = (((fg >> 16) & 255u) * a + ((bg >> 16) & 255u) * inv) >> 8;
    uint g = (((fg >> 8) & 255u) * a + ((bg >> 8) & 255u) * inv) >> 8;
    uint b = ((fg & 255u) * a + (bg & 255u) * inv) >> 8;
    return (r << 16) | (g << 8) | b;
}

// render::luma.
uint luma(uint c) {
    return (((c >> 16) & 255u) * 299u + ((c >> 8) & 255u) * 587u + (c & 255u) * 114u) / 1000u;
}

// render::glass_blend.
uint glass_blend(uint fg, uint bg, uint alpha, uint ceil_) {
    uint a = min(alpha, 256u);
    uint lf = luma(fg);
    uint lb = luma(bg);
    if (ceil_ < 255u && lb > ceil_ && lf < ceil_) {
        uint d = lb - lf;
        uint need = ((lb - ceil_) * 256u + d - 1u) / d;
        a = max(a, min(need, 256u));
    }
    return blend(fg, bg, a);
}

// GlassInk::fill.
uint ink_fill(uint fg, uint tint, uint tint_w) {
    return tint_w == 0u ? fg : blend(tint, fg, tint_w);
}

// theme::lerp_color.
uint lerp_color(uint a, uint b, uint t) {
    t = min(t, 1000u);
    uint inv = 1000u - t;
    uint r = (((a >> 16) & 255u) * inv + ((b >> 16) & 255u) * t) / 1000u;
    uint g = (((a >> 8) & 255u) * inv + ((b >> 8) & 255u) * t) / 1000u;
    uint bl = ((a & 255u) * inv + (b & 255u) * t) / 1000u;
    return (r << 16) | (g << 8) | bl;
}

// Floor of the square root of a 32-bit value: a float estimate, corrected.
uint isqrt32(uint n) {
    uint x = min(uint(sqrt(float(n))), 65535u);
    for (int i = 0; i < 4 && x * x > n; i++)
        x--;
    for (int i = 0; i < 4 && x < 65535u && (x + 1u) * (x + 1u) <= n; i++)
        x++;
    return x;
}

// Floor of the square root (render::isqrt_u64, background::isqrt).
uint isqrt(uint64_t n) {
    if (n <= 0xFFFFFFFFul)
        return isqrt32(uint(n));
    uint x = uint(sqrt(float(n)));
    for (int i = 0; i < 8 && uint64_t(x) * uint64_t(x) > n; i++)
        x--;
    for (int i = 0; i < 8 && uint64_t(x + 1u) * uint64_t(x + 1u) <= n; i++)
        x++;
    return x;
}

// render::arc_coverage_sdf.
uint arc_coverage(int cdx, int cdy, int r_q8) {
    uint64_t d2 = uint64_t(int64_t(cdx) * int64_t(cdx)) + uint64_t(int64_t(cdy) * int64_t(cdy));
    int d = int(isqrt(d2));
    int s = d - r_q8;
    if (s <= -150)
        return 256u;
    if (s >= 150)
        return 0u;
    uint t = uint((s + 150) * 256 / 300);
    uint smoothed = t * t * (768u - 2u * t) / 65536u;
    return smoothed >= 256u ? 0u : 256u - smoothed;
}

// render::rect_coverage_sdf.
uint rect_coverage(uint px, uint py, uint rx, uint ry, uint rw, uint rh, uint r) {
    if (px < rx || py < ry || px >= rx + rw || py >= ry + rh)
        return 0u;
    if (r == 0u)
        return 256u;
    int in_x = int(px - rx);
    int in_y = int(py - ry);
    int ri = int(r);
    int cdx;
    int cdy;
    if (in_x < ri)
        cdx = ri - 1 - in_x;
    else if (in_x >= int(rw) - ri)
        cdx = in_x - (int(rw) - ri);
    else
        return 256u;
    if (in_y < ri)
        cdy = ri - 1 - in_y;
    else if (in_y >= int(rh) - ri)
        cdy = in_y - (int(rh) - ri);
    else
        return 256u;
    return arc_coverage(cdx * 256 + 128, cdy * 256 + 128, ri * 256);
}

// render::rrect_distance_sdf.
int rrect_distance(uint px, uint py, uint rx, uint ry, uint rw, uint rh, uint r) {
    int pcx = int(px) * 256 + 128;
    int pcy = int(py) * 256 + 128;
    int lo_x = int(rx + r) * 256;
    int hi_x = int(rx + rw - r) * 256;
    int lo_y = int(ry + r) * 256;
    int hi_y = int(ry + rh - r) * 256;
    int64_t qx = int64_t(max(max(lo_x - pcx, pcx - hi_x), 0));
    int64_t qy = int64_t(max(max(lo_y - pcy, pcy - hi_y), 0));
    return int(isqrt(uint64_t(qx * qx + qy * qy))) - int(r) * 256;
}

// render::draw_glow_ring at one pixel.
uint glow(Blk it, uint x, uint y, uint c) {
    uint rx = BC(it, 1), ry = BC(it, 2), rw = BC(it, 3), rh = BC(it, 4);
    uint r = BC(it, 5), color = BC(it, 6), width = BC(it, 7), alpha = BC(it, 8);
    if (width == 0u || alpha == 0u || rw < 2u || rh < 2u)
        return c;
    r = min(r, min(rw / 2u, rh / 2u));
    uint x0 = rx >= width ? rx - width : 0u;
    uint y0 = ry >= width ? ry - width : 0u;
    if (x < x0 || y < y0 || x >= rx + rw + width || y >= ry + rh + width)
        return c;
    if (rect_coverage(x, y, rx, ry, rw, rh, r) > 0u)
        return c;
    int band_q8 = int(width) * 256;
    int d = max(rrect_distance(x, y, rx, ry, rw, rh, r), 0);
    if (d >= band_q8)
        return c;
    uint f = min(uint((band_q8 + 128 - d) * 256 / band_q8), 256u);
    uint a = alpha * f / 256u;
    return a > 0u ? blend(color, c, min(a, 256u)) : c;
}

// background::draw_glass_backdrop at one pixel.
uint glass_backdrop(Blk it, uint x, uint y, uint c) {
    if ((g_flags & FLAG_BLURRED) == 0u)
        return c;
    uint rx = BC(it, 1), ry = BC(it, 2), rw = BC(it, 3), rh = BC(it, 4);
    uint r = min(BC(it, 5), min(rw / 2u, rh / 2u));
    if (y < ry || y >= ry + rh)
        return c;
    uint ly = y - ry;
    uint from_edge = ly < r ? r - ly : (ly >= rh - r ? ly + 1u - (rh - r) : 0u);
    uint inset = from_edge == 0u ? 0u : r - isqrt(uint64_t(r * r - min(from_edge * from_edge, r * r)));
    uint x0 = rx + inset;
    uint x1 = rx + rw >= inset ? rx + rw - inset : 0u;
    if (x < x0 || x >= x1)
        return c;
    return g_blur.v[y * g_blur_pitch + x];
}

// render::fill_rounded_chrome_aa at one pixel.
uint chrome(Blk it, uint px, uint py, uint c) {
    uint x = BC(it, 1), y = BC(it, 2), w = BC(it, 3), h = BC(it, 4);
    uint border_a = BC(it, 5), border_b = BC(it, 6), bg = BC(it, 7);
    uint rounding = BC(it, 8), border = BC(it, 9);
    uint bo = min(BC(it, 10), 255u);
    uint go = min(BC(it, 11), 255u);
    bool paint_content = BC(it, 12) != 0u;
    if (w < 2u || h < 2u || px < x || py < y || px >= x + w || py >= y + h)
        return c;
    uint r_out = min(rounding, min(w / 2u, h / 2u));
    uint border_px = min(min(border, r_out), min(w / 2u, h / 2u));
    uint inner_x = x + border_px;
    uint inner_y = y + border_px;
    uint inner_w = w - 2u * border_px;
    uint inner_h = h - 2u * border_px;
    uint r_in = min(r_out >= border_px ? r_out - border_px : 0u, min(inner_w / 2u, inner_h / 2u));
    bool solid = border_a == border_b;
    uint ceil_ = paint_content ? g_ceil : 255u;
    uint tint_w = paint_content ? g_tint_w : 0u;
    uint glass_fill = ink_fill(bg, g_tint, tint_w);
    uint diag_max = max(w + h, 1u);

    uint straight_lo = inner_y + r_in;
    uint straight_hi = inner_y + inner_h >= r_in ? inner_y + inner_h - r_in : 0u;
    uint skip_lo = inner_x + 2u;
    uint skip_hi = inner_x + inner_w >= 2u ? inner_x + inner_w - 2u : 0u;
    if (py >= straight_lo && py < straight_hi && px >= skip_lo && px < skip_hi) {
        if (!paint_content)
            return c;
        if (go >= 255u)
            return bg;
        if ((g_flags & FLAG_GLASS) != 0u) {
            uint wpx = g_glass.v[py * g_glass_pitch + px];
            return glass_blend(glass_fill, wpx, go, ceil_);
        }
        uint bc = solid ? border_a : lerp_color(border_a, border_b, ((px - x) + (py - y)) * 1000u / diag_max);
        return glass_blend(glass_fill, blend(bc, c, bo), go, ceil_);
    }

    uint outer = rect_coverage(px, py, x, y, w, h, r_out);
    if (outer == 0u)
        return c;
    uint border_color = solid ? border_a
        : lerp_color(border_a, border_b, min(((px - x) + (py - y)) * 1000u / diag_max, 1000u));
    if (outer < 256u)
        return blend(border_color, c, min(outer * bo / 256u, 255u));
    uint inner = border_px == 0u ? 256u : rect_coverage(px, py, inner_x, inner_y, inner_w, inner_h, r_in);
    if (!paint_content)
        return inner == 256u ? c : blend(border_color, c, bo);
    if (inner == 0u)
        return blend(border_color, c, bo);
    if (inner == 256u)
        return glass_blend(glass_fill, c, go, ceil_);
    return glass_blend(glass_fill, blend(border_color, c, bo), min(go * inner / 256u, 255u), ceil_);
}

// scene::yuv_pixel: one pixel of packed YUYV as 0xFFRRGGBB; kind 2 full
// range (JFIF), else BT.601 limited.
uint yuv_pixel(uint yy, uint uu, uint vv, uint kind) {
    int y = int(yy), u = int(uu) - 128, v = int(vv) - 128;
    int r, g, b;
    if (kind == 2u) {
        int l = y << 16;
        r = (l + 91881 * v + 32768) >> 16;
        g = (l - 22554 * u - 46802 * v + 32768) >> 16;
        b = (l + 116130 * u + 32768) >> 16;
    } else {
        int c = 298 * (y - 16);
        r = (c + 409 * v + 128) >> 8;
        g = (c - 100 * u - 208 * v + 128) >> 8;
        b = (c + 516 * u + 128) >> 8;
    }
    return 0xFF000000u | (uint(clamp(r, 0, 255)) << 16) | (uint(clamp(g, 0, 255)) << 8) | uint(clamp(b, 0, 255));
}

// scene::content at one pixel; with a YUYV kind in fit bits 2..1,
// scene::content_yuv (a word is two pixels).
uint content(Blk it, uint x, uint y, uint c) {
    uint dx = BC(it, 1), dy = BC(it, 2), dw = BC(it, 3), dh = BC(it, 4);
    uint radius = BC(it, 5), fw = BC(it, 6);
    uint fit = fw & 1u, yuv = fw >> 1;
    Words src = mkaddr(BC(it, 7), BC(it, 8));
    uint sw = BC(it, 9), sh = BC(it, 10), spitch = BC(it, 11);
    if (x < dx || y < dy || x >= dx + dw || y >= dy + dh)
        return c;
    uint r = min(radius, min(dw / 2u, dh / 2u));
    uint cov = rect_coverage(x, y, dx, dy, dw, dh, r);
    if (cov == 0u)
        return c;
    uint lx = x - dx;
    uint ly = y - dy;
    uint sx = lx;
    uint sy = ly;
    uint spx = yuv != 0u ? sw * 2u : sw;
    if (fit == 0u && (spx != dw || sh != dh)) {
        sx = lx * spx / max(dw, 1u);
        sy = ly * sh / max(dh, 1u);
    }
    uint v;
    if (yuv != 0u) {
        sx = min(sx, spx - 1u);
        uint w = src.v[min(sy, sh - 1u) * spitch + sx / 2u];
        uint luma = (sx & 1u) == 0u ? (w & 255u) : ((w >> 16) & 255u);
        v = yuv_pixel(luma, (w >> 8) & 255u, w >> 24, yuv);
    } else {
        v = src.v[min(sy, sh - 1u) * spitch + min(sx, sw - 1u)];
    }
    return cov >= 256u ? v : blend(v, c, cov);
}

// scene::panel at one pixel; the base is the blurred pixel beneath.
uint panel(Blk it, uint x, uint y, uint c) {
    uint dx = BC(it, 1), dy = BC(it, 2), dw = BC(it, 3), dh = BC(it, 4);
    Words src = mkaddr(BC(it, 5), BC(it, 6));
    uint sw = BC(it, 7), sh = BC(it, 8), spitch = BC(it, 9);
    if (x < dx || y < dy || x >= dx + dw || y >= dy + dh || x >= dx + sw || y >= dy + sh)
        return c;
    uint pv = src.v[(y - dy) * spitch + (x - dx)];
    uint a = pv >> 24;
    if (a == 0u)
        return c;
    if (a < 255u) {
        uint base = c;
        if (PANEL_BSW != 0u) {
            // background::blur_region's bilinear step.
            uint fy = (y >= 2u ? y - 2u : 0u) * 256u / 4u;
            uint gy = (fy >> 8) >= PANEL_BY0 ? (fy >> 8) - PANEL_BY0 : 0u;
            uint y0 = min(gy, PANEL_BSH - 1u);
            uint wy = fy & 255u;
            uint y1 = min(y0 + 1u, PANEL_BSH - 1u);
            uint fx = (x >= 2u ? x - 2u : 0u) * 256u / 4u;
            uint gx = (fx >> 8) >= PANEL_BX0 ? (fx >> 8) - PANEL_BX0 : 0u;
            uint x0 = min(gx, PANEL_BSW - 1u);
            uint wx = fx & 255u;
            uint x1 = min(x0 + 1u, PANEL_BSW - 1u);
            uint q00 = PANEL_PLANES.v[y0 * PANEL_BSW + x0];
            uint q01 = PANEL_PLANES.v[y0 * PANEL_BSW + x1];
            uint q10 = PANEL_PLANES.v[y1 * PANEL_BSW + x0];
            uint q11 = PANEL_PLANES.v[y1 * PANEL_BSW + x1];
            base = 0u;
            for (uint k = 0u; k < 3u; k++) {
                uint sh_ = 16u - 8u * k;
                uint top = ((q00 >> sh_) & 255u) * (256u - wx) + ((q01 >> sh_) & 255u) * wx;
                uint bot = ((q10 >> sh_) & 255u) * (256u - wx) + ((q11 >> sh_) & 255u) * wx;
                uint v = (top * (256u - wy) + bot * wy) >> 16;
                base |= min(v, 255u) << sh_;
            }
        }
        uint fill = ink_fill(pv & 0xFFFFFFu, g_tint, g_tint_w);
        return glass_blend(fill, base, a, g_ceil);
    }
    return blend(pv & 0xFFFFFFu, c, a);
}

// scene::text at one pixel.
uint text(Blk it, uint x, uint y, uint c) {
    uint dx = BC(it, 1), dy = BC(it, 2), dw = BC(it, 3), dh = BC(it, 4);
    Words src = mkaddr(BC(it, 5), BC(it, 6));
    uint sw = BC(it, 7), sh = BC(it, 8), spitch = BC(it, 9);
    if (x < dx || y < dy || x >= dx + dw || y >= dy + dh || x >= dx + sw || y >= dy + sh)
        return c;
    uint pv = src.v[(y - dy) * spitch + (x - dx)];
    // The alpha byte is glyph coverage (0..255), `blend` takes 0..256.
    uint a = pv >> 24;
    if (a == 0u)
        return c;
    return blend(pv & 0xFFFFFFu, c, a + (a >> 7u));
}

// Item::Fill at one pixel.
uint fill(Blk it, uint x, uint y, uint c) {
    uint rx = BC(it, 1), ry = BC(it, 2), rw = BC(it, 3), rh = BC(it, 4);
    uint color = BC(it, 5), alpha = BC(it, 6), radius = BC(it, 7);
    if (alpha == 0u || x < rx || y < ry || x >= rx + rw || y >= ry + rh)
        return c;
    uint r = min(radius, min(rw / 2u, rh / 2u));
    uint cov = rect_coverage(x, y, rx, ry, rw, rh, r);
    if (cov == 0u)
        return c;
    uint a = min(min(alpha, 255u) * cov / 255u, 256u);
    if (a == 0u)
        return c;
    return a >= 256u ? color & 0xFFFFFFu : blend(color & 0xFFFFFFu, c, a);
}

// Item::CloseGlyph at one pixel.
uint close_glyph(Blk it, uint x, uint y, uint c) {
    uint rx = BC(it, 1), ry = BC(it, 2), rw = BC(it, 3), rh = BC(it, 4);
    uint asz = BC(it, 5), color = BC(it, 6), off = BC(it, 7);
    if (x < rx || y < ry || x >= rx + rw || y >= ry + rh)
        return c;
    uint ox = rx + (rw >= asz ? rw - asz : 0u) / 2u;
    uint oy = ry + (rh >= asz ? rh - asz : 0u) / 2u;
    if (x < ox || y < oy || x >= ox + asz || y >= oy + asz)
        return c;
    uint b = off + (y - oy) * asz + (x - ox);
    uint a = (g_glyphs.v[b >> 2] >> ((b & 3u) * 8u)) & 255u;
    if (a == 0u)
        return c;
    a += a >> 7;
    return a >= 256u ? color : blend(color, c, a);
}

