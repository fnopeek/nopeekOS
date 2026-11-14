#version 460
// The rectangle of one draw as a triangle strip of four vertices, from
// pixel coordinates to the viewport's normalized ones.
#extension GL_GOOGLE_include_directive : require

#include "draw.h.glsl"

void main() {
    uint x = (gl_VertexIndex & 1) != 0 ? p.rect[2] : p.rect[0];
    uint y = (gl_VertexIndex & 2) != 0 ? p.rect[3] : p.rect[1];
    gl_Position = vec4(float(x) * 2.0 / float(p.width) - 1.0, float(y) * 2.0 / float(p.height) - 1.0, 0.0, 1.0);
}
