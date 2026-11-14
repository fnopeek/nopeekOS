#version 460
// One scene item over the pixels of one draw: the colour the tile buffer
// holds for the pixel (VK_EXT_shader_tile_image), painted over by the item
// as compose.comp paints it, written back. The attachment is R32_UINT, one
// XRGB8888 word per pixel, so the mixing is the CPU executor's, exactly.
// KIND selects the item kind at compile time: one small program per kind.
#extension GL_EXT_buffer_reference : require
#extension GL_EXT_shader_explicit_arithmetic_types_int64 : require
#extension GL_EXT_shader_tile_image : require
#extension GL_GOOGLE_include_directive : require

layout(constant_id = 0) const uint KIND = 0u;

layout(location = 0) tileImageEXT highp uattachmentEXT dst;
layout(location = 0) out uint color;

#include "draw.h.glsl"

layout(buffer_reference, std430, buffer_reference_align = 4) buffer Words {
    uint v[];
};

#define Blk uint[16]
#define BC(b, k) (b)[k]

const uint FLAG_WALL = 1u;
const uint FLAG_BLURRED = 2u;
const uint FLAG_GLASS = 4u;

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
    uint c;
    if (KIND == 1u) {
        c = (g_flags & FLAG_WALL) != 0u ? g_wall.v[y * g_wall_pitch + x] : 0xFF181820u;
    } else {
        c = colorAttachmentReadEXT(dst).x;
        switch (KIND) {
        case 2u: c = glow(it, x, y, c); break;
        case 3u: c = glass_backdrop(it, x, y, c); break;
        case 4u: c = chrome(it, x, y, c); break;
        case 5u: c = content(it, x, y, c); break;
        case 6u: c = panel(it, x, y, c); break;
        case 7u: c = text(it, x, y, c); break;
        case 8u: c = fill(it, x, y, c); break;
        case 9u: c = close_glyph(it, x, y, c); break;
        default: break;
        }
    }
    color = c;
}
