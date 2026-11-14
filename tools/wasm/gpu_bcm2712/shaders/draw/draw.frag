#version 460
// One scene item over the pixels of one draw: what the item does to the
// pixel (items.glsl `Op`), applied to the colour the tile buffer holds for
// it (VK_EXT_shader_tile_image) as compose.comp paints it, written back. The attachment is R32_UINT, one
// XRGB8888 word per pixel, so the mixing is the CPU executor's, exactly.
// KIND selects the item kind at compile time: one small program per kind,
// 10 for the straight-row part of a chrome item over glass (items.glsl
// `chrome_glass`), and 11 for that part with a text item over it, mixed in
// one pass (the window's inside).
#extension GL_EXT_buffer_reference : require
#extension GL_EXT_shader_explicit_arithmetic_types_int64 : require
#extension GL_EXT_shader_tile_image : require
#extension GL_GOOGLE_include_directive : require

layout(constant_id = 0) const uint KIND = 0u;

layout(location = 0) tileImageEXT highp uattachmentEXT dst;
layout(location = 0) out uint color;
// rect.vert: a chrome item's glass fill and its luma, for kinds 10 and 11.
layout(location = 0) flat in uint v_fill;
layout(location = 1) flat in uint v_fill_luma;

#include "draw.h.glsl"

layout(buffer_reference, std430, buffer_reference_align = 4) buffer Words {
    uint v[];
};

#define Blk uint[16]
#define BC(b, k) (b)[k]

const uint FLAG_WALL = 1u;
const uint FLAG_BLURRED = 2u;
const uint FLAG_GLASS = 4u;

#define ITEM_OPS
#define V3D_ARITH
#include "../../../gpu_shaders/items.glsl"

void main() {
    uint x = uint(gl_FragCoord.x);
    uint y = uint(gl_FragCoord.y);
    uint it[16] = p.item;
    g_flags = p.flags;
    g_ceil = p.ceil_;
    g_tint = p.tint;
    g_tint_w = p.tint_w;
    Words src = mkaddr(p.src_lo, p.src_hi);
    g_wall = src;
    g_wall_pitch = p.src_pitch;
    g_blur = src;
    g_blur_pitch = p.src_pitch;
    g_glass = src;
    g_glass_pitch = p.src_pitch;
    g_glyphs = src;
    if (KIND == 1u) {
        color = (g_flags & FLAG_WALL) != 0u ? g_wall.v[y * g_wall_pitch + x] : 0xFF181820u;
        return;
    }
    // The item's own loads and arithmetic first, the tile buffer last: a
    // read before them would hold the scoreboard through all of it.
    Op o = op_keep();
    Op o2 = op_keep();
    switch (KIND) {
    case 2u: o = glow(it, x, y); break;
    case 3u: o = glass_backdrop(it, x, y); break;
    case 4u: o = chrome(it, x, y); break;
    case 5u: o = content(it, x, y); break;
    case 6u: o = panel(it, x, y); break;
    case 7u: o = text(it, x, y); break;
    case 8u: o = fill(it, x, y); break;
    case 9u: o = close_glyph(it, x, y); break;
    case 10u: o = chrome_glass(x, y, v_fill, v_fill_luma, min(it[11], 255u)); break;
    case 11u:
        o = chrome_glass(x, y, v_fill, v_fill_luma, min(it[11], 255u));
        o2 = text_inside(p.item2, x, y);
        break;
    default: break;
    }
    uint c = apply(o, colorAttachmentReadEXT(dst).x);
    color = KIND == 11u ? apply(o2, c) : c;
}
