#version 460
// The rectangle of one draw as a triangle strip of four vertices, from
// pixel coordinates to the viewport's normalized ones, and what a chrome
// item's glass (kinds 10 and 11, items.glsl `chrome_glass`) takes from it
// once for the draw: its tinted fill and that fill's luma.
#extension GL_EXT_buffer_reference : require
#extension GL_EXT_shader_explicit_arithmetic_types_int64 : require
#extension GL_GOOGLE_include_directive : require

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

layout(location = 0) flat out uint v_fill;
layout(location = 1) flat out uint v_fill_luma;

void main() {
    uint x = (gl_VertexIndex & 1) != 0 ? p.rect[2] : p.rect[0];
    uint y = (gl_VertexIndex & 2) != 0 ? p.rect[3] : p.rect[1];
    gl_Position = vec4(float(x) * 2.0 / float(p.width) - 1.0, float(y) * 2.0 / float(p.height) - 1.0, 0.0, 1.0);
    // `chrome`: the glass fill of an item that paints its content.
    uint tint_w = p.item[12] != 0u ? p.tint_w : 0u;
    v_fill = ink_fill(p.item[7], p.tint, tint_w);
    v_fill_luma = luma(v_fill);
}
