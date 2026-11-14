// The push constants of one draw: the rectangle it covers, the item it
// paints (compose.rs ITEM_WORDS) and the frame values that item reads.
layout(push_constant, std430) uniform Draw {
    // x0, y0, x1, y1 in pixels; 0..4.
    uint rect[4];
    // The item's 16 words; word 0 is its kind.
    uint item[16];
    // compose.rs F_FLAGS, F_INK_CEIL, F_INK_TINT, F_INK_TINT_W.
    uint flags;
    uint ceil_;
    uint tint;
    uint tint_w;
    // The source the item kind reads beside its own: the wallpaper, the
    // blurred wallpaper, the glass or the glyphs, and its pitch in words.
    uint src_lo;
    uint src_hi;
    uint src_pitch;
    // The viewport in pixels.
    uint width;
    uint height;
} p;
