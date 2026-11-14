//! One render pass as v3dv records it for V3D 7.1 (Mesa 26.2.4
//! `src/broadcom/vulkan`), for the drawing model of gpu_bcm2712: one
//! R32_UINT colour attachment, a raster (linear) image loaded and stored in
//! layout GENERAL, no depth or stencil, one sample, one layer, a render area
//! inside the image; per draw a graphics pipeline (vertex, coordinate and
//! fragment program), 128 bytes of push constants for the vertex and
//! fragment stages, and `vkCmdDraw(n, 1, 0, 0)` of a triangle strip without
//! vertex buffers; the viewport, scissor and the rest of the fixed function
//! state are static and the same in every pipeline (no blending, cull none,
//! counter-clockwise front faces, line width 1).
//!
//! [`record`] builds the binning list, the render list and the indirect
//! buffer (uniform streams, shader state records, the generic tile list)
//! byte for byte as v3dv does: the same packets, the same dirty-state
//! decisions per draw, the same buffer growth and chaining. The caller owns
//! memory ([`BoAlloc`]) and writes each uniform stream's words when v3dv
//! would ([`record`]'s `fill`).

use crate::v3d::BinJob;
use crate::v3d_packet_gen::field_values::*;
use crate::v3d_packet_gen::*;
use alloc::vec;
use alloc::vec::Vec;

/// `v3d_device_info.cle_readahead` on V3D 7.1.
const CLE_READAHEAD: u32 = 1024;
/// `V3D_CL_MAX_GROW_SIZE`.
const CL_MAX_GROW_SIZE: u32 = 1024 * 1024;
/// `devinfo->page_size`, and `v3dv_bo_alloc`'s alignment.
pub const PAGE_SIZE: u32 = 4096;
/// `V3D_TILE_ALLOC_INITIAL_BLOCK_SIZE` and the packets' encoding of it.
const TILE_ALLOC_INITIAL_BLOCK_SIZE: u32 = 128;
const TILE_ALLOC_INITIAL_BLOCK_SIZE_ENUM: u32 = TILE_ALLOC_INITIAL_BLOCK_SIZE >> 7;
/// `V3D_TILE_ALLOC_OVERFLOW_BLOCK_SIZE_ENUM` (64 >> 7).
const TILE_ALLOC_OVERFLOW_BLOCK_SIZE_ENUM: u32 = 0;
/// `V3D_MAX_SUPERTILES`.
const MAX_SUPERTILES: u32 = 256;
/// `V3D_MAX_RENDER_TARGETS(71)`.
const MAX_RENDER_TARGETS: u32 = 8;
/// `V3D_MAX_SAMPLES`.
const MAX_SAMPLES: u32 = 4;
/// `devinfo->clipper_xy_granularity` on V3D 7.1.
pub const CLIPPER_XY_GRANULARITY: f32 = 64.0;
/// `maxPushConstantsSize`, in words.
pub const PUSH_WORDS: usize = 32;
/// `ARRAY_SIZE(v3d_fs_prog_data.flat_shade_flags)` (64 inputs, 24 per word).
pub const VARYING_FLAG_WORDS: usize = 3;

// VK_FORMAT_R32_UINT in v3dvx_formats.c: output format R32UI, no swizzle;
// v3dX(get_internal_type_bpp_for_output_format): internal type 32UI, 32 bpp.
const RT_OUTPUT_FORMAT: u32 = output_image_format::R32UI;
const RT_INTERNAL_TYPE: u32 = internal_type::_32UI;
const RT_INTERNAL_BPP: u32 = internal_bpp::_32;

/// What a buffer is for, so the allocator can tell the job's buffers apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoKind {
    Bcl,
    Rcl,
    Indirect,
    TileAlloc,
    TileState,
}

/// GPU memory for the job (`v3dv_bo_alloc`): `size` bytes, a multiple of
/// [`PAGE_SIZE`], mapped at the returned V3D address; `None` when there is
/// none.
pub trait BoAlloc {
    fn alloc(&mut self, kind: BoKind, size: u32) -> Option<u32>;
}

/// A buffer of a control list or of the indirect state: its V3D address and
/// its whole contents (zero where nothing was written).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bo {
    pub va: u32,
    pub bytes: Vec<u8>,
}

/// A buffer the job only references: address and size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoRef {
    pub va: u32,
    pub size: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutOfMemory;

/// A rectangle in pixels (`VkRect2D` with a non-negative offset).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// `VkViewport`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

/// The colour attachment: a linear R32_UINT image of `width` x `height`
/// pixels at V3D address `va`, `stride` bytes per row (its slice's stride).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Image {
    pub va: u32,
    pub stride: u32,
    pub width: u32,
    pub height: u32,
}

/// The fields of `struct v3d_prog_data` the job reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProgData {
    pub threads: u32,
    pub single_seg: bool,
    pub tmu_dirty_rcl: bool,
    /// `uniforms.count`: the words of the program's uniform stream.
    pub uniform_count: u32,
}

/// The fields of `struct v3d_vs_prog_data` the job reads (vertex and
/// coordinate shader).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VsProgData {
    pub base: ProgData,
    pub uses_iid: bool,
    pub uses_biid: bool,
    pub uses_vid: bool,
    pub vpm_input_size: u32,
    pub vpm_output_size: u32,
    pub vcm_cache_size: u32,
    pub writes_psiz: bool,
}

/// The fields of `struct v3d_fs_prog_data` the job reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FsProgData {
    pub base: ProgData,
    pub uses_pid: bool,
    pub flat_shade_flags: [u32; VARYING_FLAG_WORDS],
    pub noperspective_flags: [u32; VARYING_FLAG_WORDS],
    pub centroid_flags: [u32; VARYING_FLAG_WORDS],
    pub num_inputs: u32,
    pub writes_z: bool,
    pub disable_ez: bool,
    pub uses_center_w: bool,
    pub uses_implicit_point_line_varyings: bool,
    pub lock_scoreboard_on_first_thrsw: bool,
}

/// A compiled program (`struct v3dv_shader_variant`): the V3D address of
/// its code and its prog_data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Variant<P> {
    pub code: u32,
    pub prog: P,
}

/// A graphics pipeline of the drawing model: its three programs and the
/// colour attachment's `VkColorComponentFlags` write mask.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pipeline {
    pub vs: Variant<VsProgData>,
    /// The binning (coordinate) shader v3dv builds from the vertex shader.
    pub cs: Variant<VsProgData>,
    pub fs: Variant<FsProgData>,
    pub color_write_mask: u32,
}

/// One `vkCmdBindPipeline` + `vkCmdPushConstants` + `vkCmdDraw`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    /// Index into [`Pass::pipelines`].
    pub pipeline: usize,
    pub push: [u32; PUSH_WORDS],
    pub vertex_count: u32,
}

/// The render pass: the device revision (V3D 7.1.`rev`), the attachment,
/// the render area, the pipelines' static viewport and scissor, and the
/// draws in order.
pub struct Pass<'a> {
    pub rev: u32,
    pub image: Image,
    pub render_area: Rect,
    pub viewport: Viewport,
    pub scissor: Rect,
    pub pipelines: &'a [Pipeline],
    pub draws: &'a [Draw],
}

/// The program a uniform stream belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Fragment,
    Vertex,
    /// The vertex shader's binning variant (`BROADCOM_SHADER_VERTEX_BIN`).
    Coordinate,
}

/// What `drm_v3d_submit_cl` carries for the job (`handle_cl_job`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubmitCl {
    pub bcl_start: u32,
    pub bcl_end: u32,
    pub rcl_start: u32,
    pub rcl_end: u32,
    pub qma: u32,
    pub qms: u32,
    pub qts: u32,
    /// `DRM_V3D_SUBMIT_CL_FLUSH_CACHE`: a program wrote through the TMU.
    pub flush_cache: bool,
}

/// The recorded job: the buffers of each list in allocation order (the
/// lists start in the first and chain on by `BRANCH`; the indirect buffers
/// are only referenced), the tile allocation and tile state memory, and
/// the submit parameters.
#[derive(Clone, Debug)]
pub struct Job {
    pub bcl: Vec<Bo>,
    pub rcl: Vec<Bo>,
    pub indirect: Vec<Bo>,
    pub tile_alloc: BoRef,
    pub tile_state: BoRef,
    pub submit: SubmitCl,
}

impl Job {
    /// The binner half of the submit, for `v3d::bin_job_run`.
    pub fn bin_job(&self) -> BinJob {
        BinJob {
            start: self.submit.bcl_start,
            end: self.submit.bcl_end,
            qma: self.submit.qma,
            qms: self.submit.qms,
            qts: self.submit.qts,
        }
    }

    /// The render list's start and end, for `v3d::render_job_run`.
    pub fn render_list(&self) -> (u32, u32) {
        (self.submit.rcl_start, self.submit.rcl_end)
    }
}

fn align(v: u32, a: u32) -> u32 {
    v.div_ceil(a) * a
}

/// `v3d_device_has_draw_index`: V3D 7.1.10 (BCM2712 D0) and later.
fn has_draw_index(rev: u32) -> bool {
    rev >= 10
}

/// Emits one generated packet into a [`Cl`].
macro_rules! emit {
    ($cl:expr, $t:ident { $($f:tt)* }) => {{
        let p = $t { $($f)* };
        $cl.put($t::LENGTH, |b| p.pack(b));
    }};
}

/// `struct v3dv_cl`: a list growing over buffers; `next` is the offset of
/// the next byte in the last buffer, `size` its usable bytes.
struct Cl {
    kind: BoKind,
    bos: Vec<Bo>,
    next: u32,
    size: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Chain {
    None,
    Branch,
}

impl Cl {
    fn new(kind: BoKind) -> Cl {
        Cl { kind, bos: Vec::new(), next: 0, size: 0 }
    }

    /// `v3dv_cl_offset`.
    fn offset(&self) -> u32 {
        self.next
    }

    /// `v3dv_cl_get_address`.
    fn address(&self) -> u32 {
        self.bo_va() + self.next
    }

    /// The current buffer's address (`cl->bo->offset`).
    fn bo_va(&self) -> u32 {
        self.bos.last().map_or(0, |b| b.va)
    }

    fn put(&mut self, len: usize, pack: impl FnOnce(&mut [u8])) {
        let at = self.next as usize;
        let bo = self.bos.last_mut().expect("v3d_draw: a packet into a list without a buffer");
        pack(&mut bo.bytes[at..at + len]);
        self.next += len as u32;
        debug_assert!(self.next <= self.size);
    }

    /// `cl_emit_prepacked` and `cl_emit_with_prepacked`: the packet's bytes
    /// ORed over the pipeline's.
    fn put_with_prepacked(&mut self, prepacked: &[u8], pack: impl FnOnce(&mut [u8])) {
        let mut b = vec![0u8; prepacked.len()];
        pack(&mut b);
        self.put(prepacked.len(), |out| {
            for (o, (x, y)) in out.iter_mut().zip(b.iter().zip(prepacked)) {
                *o = x | y;
            }
        });
    }

    /// `cl_alloc_bo`.
    fn alloc_bo(&mut self, space: u32, chain: Chain, a: &mut dyn BoAlloc) -> Result<(), OutOfMemory> {
        let unusable = match chain {
            Chain::Branch => CLE_READAHEAD + Branch::LENGTH as u32,
            Chain::None => 0,
        };
        let mut space = align(space + unusable, PAGE_SIZE);
        if let Some(bo) = self.bos.last() {
            space = (bo.bytes.len() as u32 * 2).min(CL_MAX_GROW_SIZE).max(space);
        }
        let size = align(space, PAGE_SIZE);
        let va = a.alloc(self.kind, size).ok_or(OutOfMemory)?;
        if !self.bos.is_empty() && chain == Chain::Branch {
            self.size += Branch::LENGTH as u32;
            emit!(self, Branch { address: va });
        }
        self.bos.push(Bo { va, bytes: vec![0; size as usize] });
        self.size = size - unusable;
        self.next = 0;
        Ok(())
    }

    /// `v3dv_cl_ensure_space`: room for `space` bytes at `alignment`, in a
    /// new unchained buffer if the current one is full; the offset.
    fn ensure_space(&mut self, space: u32, alignment: u32, a: &mut dyn BoAlloc) -> Result<u32, OutOfMemory> {
        let offset = align(self.offset(), alignment);
        if offset + space <= self.size {
            self.next = offset;
            return Ok(offset);
        }
        self.alloc_bo(space, Chain::None, a)?;
        Ok(0)
    }

    /// `v3dv_cl_ensure_space_with_branch`.
    fn ensure_space_with_branch(&mut self, space: u32, a: &mut dyn BoAlloc) -> Result<(), OutOfMemory> {
        if self.offset() + space <= self.size {
            return Ok(());
        }
        self.alloc_bo(space, Chain::Branch, a)
    }
}

/// `struct v3dv_frame_tiling`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameTiling {
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub render_target_count: u32,
    pub internal_bpp: u32,
    pub total_color_bpp: u32,
    pub msaa: bool,
    pub tile_width: u32,
    pub tile_height: u32,
    pub draw_tiles_x: u32,
    pub draw_tiles_y: u32,
    pub supertile_width: u32,
    pub supertile_height: u32,
    pub frame_width_in_supertiles: u32,
    pub frame_height_in_supertiles: u32,
}

/// `tile_size_valid` (v3d_util.c), with V3D 7.1's TLB sizes.
fn tile_size_valid(pixel_count: u32, color_bpp: u32, depth_bpp: u32) -> bool {
    const TLB_COLOR_SIZE: u32 = 16 * 1024;
    const TLB_DEPTH_SIZE: u32 = 16 * 1024;
    const TLB_AUX_DEPTH_SIZE: u32 = 8 * 1024;
    if pixel_count * depth_bpp <= TLB_AUX_DEPTH_SIZE && pixel_count * color_bpp <= TLB_COLOR_SIZE + TLB_DEPTH_SIZE {
        return true;
    }
    pixel_count * depth_bpp <= TLB_DEPTH_SIZE && pixel_count * color_bpp <= TLB_COLOR_SIZE
}

/// `v3d_choose_tile_size` for V3D 7.x without double-buffer: width, height.
pub fn choose_tile_size(total_color_bpp: u32, msaa: bool) -> (u32, u32) {
    const TILE_SIZES: [(u32, u32); 7] = [(64, 64), (64, 32), (32, 32), (32, 16), (16, 16), (16, 8), (8, 8)];
    let color_bpp = total_color_bpp * if msaa { 4 } else { 1 };
    let depth_bpp = 4 * if msaa { 4 } else { 1 };
    let mut idx = 0;
    loop {
        let (w, h) = TILE_SIZES[idx];
        if tile_size_valid(w * h, color_bpp, depth_bpp) {
            break;
        }
        idx += 1;
        if idx == TILE_SIZES.len() {
            break;
        }
    }
    assert!(idx < TILE_SIZES.len());
    TILE_SIZES[idx]
}

/// `v3d_internal_bpp_words`.
fn internal_bpp_words(internal_bpp: u32) -> u32 {
    match internal_bpp {
        internal_bpp::_32 => 1,
        internal_bpp::_64 => 2,
        internal_bpp::_128 => 4,
        _ => unreachable!("v3d_draw: internal bpp {}", internal_bpp),
    }
}

/// `v3dX(clamp_for_format_and_type)` on V3D 7.1 for a format that is
/// neither snorm nor sRGB.
fn clamp_for_format_and_type(rt_type: u32) -> u32 {
    use render_target_type_clamp as c;
    match rt_type {
        internal_type::_8I => c::_8I_CLAMPED,
        internal_type::_8UI => c::_8UI_CLAMPED,
        internal_type::_8 => c::_8,
        internal_type::_16I => c::_16I_CLAMPED,
        internal_type::_16UI => c::_16UI_CLAMPED,
        internal_type::_16F => c::_16F,
        internal_type::_32I => c::_32I_CLAMPED,
        internal_type::_32UI => c::_32UI_CLAMPED,
        internal_type::_32F => c::_32F,
        _ => unreachable!("v3d_draw: internal type {}", rt_type),
    }
}

/// `log2_tile_size`.
fn log2_tile_size(size: u32) -> u32 {
    match size {
        8 => 0,
        16 => 1,
        32 => 2,
        64 => 3,
        _ => unreachable!("v3d_draw: tile size {}", size),
    }
}

/// `job_compute_frame_tiling` without double-buffer.
pub fn compute_frame_tiling(width: u32, height: u32, layers: u32, render_target_count: u32, internal_bpp: u32,
                            total_color_bpp: u32, msaa: bool) -> FrameTiling {
    let (tile_width, tile_height) = choose_tile_size(total_color_bpp, msaa);
    let mut t = FrameTiling {
        width,
        height,
        layers,
        render_target_count,
        internal_bpp,
        total_color_bpp,
        msaa,
        tile_width,
        tile_height,
        draw_tiles_x: width.div_ceil(tile_width),
        draw_tiles_y: height.div_ceil(tile_height),
        supertile_width: 1,
        supertile_height: 1,
        ..Default::default()
    };
    loop {
        t.frame_width_in_supertiles = t.draw_tiles_x.div_ceil(t.supertile_width);
        t.frame_height_in_supertiles = t.draw_tiles_y.div_ceil(t.supertile_height);
        let num_supertiles = t.frame_width_in_supertiles * t.frame_height_in_supertiles;
        // The hardware takes up to 256 supertiles, but not 1x256 or 256x1.
        if t.frame_width_in_supertiles < MAX_SUPERTILES && t.frame_height_in_supertiles < MAX_SUPERTILES
            && num_supertiles <= MAX_SUPERTILES
        {
            break;
        }
        if t.supertile_width < t.supertile_height {
            t.supertile_width += 1;
        } else {
            t.supertile_height += 1;
        }
    }
    t
}

/// `v3d_tile_alloc_sizes`: tile allocation and tile state bytes.
pub fn tile_alloc_sizes(layers: u32, tiles_x: u32, tiles_y: u32, draws: u32) -> (u32, u32) {
    let tiles_size = layers * tiles_x * tiles_y * TILE_ALLOC_INITIAL_BLOCK_SIZE;
    // The PTB allocates in aligned 4 KiB chunks after the initial blocks,
    // and the first two chunks never raise OOM.
    let mut alloc_size = align(tiles_size, 4096);
    alloc_size += 8192;
    alloc_size += (tiles_size * draws / 2).min(512 * 1024);
    alloc_size = align(alloc_size, PAGE_SIZE);
    (alloc_size, layers * tiles_x * tiles_y * 256)
}

/// `v3d_compute_rt_row_row_stride_128_bits`.
fn rt_row_row_stride_128_bits(tile_width: u32, bpp_words: u32) -> u32 {
    tile_width * bpp_words / 2
}

/// `struct vpm_config`'s fields without a geometry shader.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VpmConfig {
    pub a_s: u32,
    pub v_e: u32,
    pub v_c: u32,
}

/// `v3d_compute_vpm_config` without a geometry shader: binning, render.
pub fn compute_vpm_config(vs_bin: &VsProgData, vs: &VsProgData) -> (VpmConfig, VpmConfig) {
    (VpmConfig { a_s: 1, v_e: 0, v_c: vs_bin.vcm_cache_size }, VpmConfig { a_s: 1, v_e: 0, v_c: vs.vcm_cache_size })
}

/// `v3dX(viewport_compute_xform)`: scale, translate.
pub fn viewport_compute_xform(vp: &Viewport) -> ([f32; 3], [f32; 3]) {
    let half_width = 0.5f32 * vp.width;
    let half_height = 0.5f32 * vp.height;
    let n = vp.min_depth as f64;
    let f = vp.max_depth as f64;
    ([half_width, half_height, (f - n) as f32], [half_width + vp.x, half_height + vp.y, n as f32])
}

/// `v3dv_write_uniforms` for the uniform contents of the drawing model's
/// programs: a constant, a push-constant word, or the viewport's X or Y
/// scale (in clipper units), Z offset or Z scale. False for any other
/// contents (the generator refuses them) or a list that does not fit.
pub fn write_uniforms(list: &[crate::shaders_gen::U], push: &[u32; PUSH_WORDS], vp: &Viewport, out: &mut [u32])
                      -> bool {
    use crate::shaders_gen::U;
    if list.len() != out.len() {
        return false;
    }
    let (scale, translate) = viewport_compute_xform(vp);
    for (w, u) in out.iter_mut().zip(list) {
        *w = match *u {
            U::Const(v) => v,
            U::Push(n) => match push.get(n as usize) {
                Some(&v) => v,
                None => return false,
            },
            U::ViewportXScale => (scale[0] * CLIPPER_XY_GRANULARITY).to_bits(),
            U::ViewportYScale => (scale[1] * CLIPPER_XY_GRANULARITY).to_bits(),
            U::ViewportZOffset => translate[2].to_bits(),
            U::ViewportZScale => scale[2].to_bits(),
            U::WgBase(_) | U::NumWg(_) | U::SpillOffset | U::SpillSizePerThread => return false,
        };
    }
    true
}

/// The pipeline state v3dv packs once per pipeline.
struct Packed {
    /// `pack_shader_state_record`.
    shader_state_record: Vec<u8>,
    /// `pack_vcm_cache_size`.
    vcm_cache_size: [u8; VCMCacheSize::LENGTH],
    /// `pack_cfg_bits`.
    cfg_bits: [u8; CfgBits::LENGTH],
    vpm_cfg_bin: VpmConfig,
    vpm_cfg: VpmConfig,
    /// `pipeline->blend.color_write_masks` (`pack_blend`).
    color_write_masks: u32,
    /// `pipeline->sample_mask` (`pipeline_set_sample_mask`).
    sample_mask: u32,
    /// `pipeline->va_count`: no vertex attributes in this model.
    va_count: u32,
}

/// `v3dX(pipeline_pack_state)` and `v3dX(pipeline_pack_compile_state)` for
/// the model's fixed state.
fn pack_pipeline(p: &Pipeline, rev: u32) -> Packed {
    let (vpm_cfg_bin, vpm_cfg) = compute_vpm_config(&p.cs.prog, &p.vs.prog);

    // pack_blend: blending off, one attachment.
    let color_write_masks = !p.color_write_mask & 0xf;
    let blend_enables = 0;

    // pack_cfg_bits: fill mode, no MSAA, the default line mode, provoking
    // vertex first, depth clip on (no depth clamp).
    let mut cfg_bits = [0u8; CfgBits::LENGTH];
    let line_rasterization = line_rasterization::PERP_END_CAPS;
    CfgBits {
        line_rasterization,
        rasterizer_oversample_mode: 0,
        direct3d_provoking_vertex: true,
        blend_enable: blend_enables != 0,
        z_clipping_mode: z_clip_mode::ZERO_TO_ONE,
        z_clamp_mode: false,
        ..Default::default()
    }
    .pack(&mut cfg_bits);

    let fs = &p.fs.prog;
    let vs = &p.vs.prog;
    let cs = &p.cs.prog;
    let point_size_in_shaded_vertex_data = vs.writes_psiz;
    let mut shader_state_record = vec![0u8; GLShaderStateRecord::LENGTH];
    if has_draw_index(rev) {
        shader_state_record.resize(GLShaderStateRecordDrawIndex::LENGTH, 0);
        GLShaderStateRecordDrawIndex {
            enable_clipping: true,
            point_size_in_shaded_vertex_data,
            fragment_shader_does_z_writes: fs.writes_z,
            turn_off_early_z_test: fs.disable_ez,
            fragment_shader_uses_real_pixel_centre_w_in_addition_to_centroid_w2: fs.uses_center_w,
            enable_sample_rate_shading: false,
            any_shader_reads_hardware_written_primitive_id: false,
            do_scoreboard_wait_on_first_thread_switch: fs.lock_scoreboard_on_first_thrsw,
            disable_implicit_point_line_varyings: !fs.uses_implicit_point_line_varyings,
            number_of_varyings_in_fragment_shader: fs.num_inputs,
            coordinate_shader_input_vpm_segment_size: cs.vpm_input_size,
            vertex_shader_input_vpm_segment_size: vs.vpm_input_size,
            coordinate_shader_output_vpm_segment_size: cs.vpm_output_size,
            vertex_shader_output_vpm_segment_size: vs.vpm_output_size,
            min_coord_shader_input_segments_required_in_play: vpm_cfg_bin.a_s,
            min_vertex_shader_input_segments_required_in_play: vpm_cfg.a_s,
            min_coord_shader_output_segments_required_in_play_in_addition_to_vcm_cache_size: vpm_cfg_bin.v_e,
            min_vertex_shader_output_segments_required_in_play_in_addition_to_vcm_cache_size: vpm_cfg.v_e,
            coordinate_shader_4_way_threadable: cs.base.threads == 4,
            vertex_shader_4_way_threadable: vs.base.threads == 4,
            fragment_shader_4_way_threadable: fs.base.threads == 4,
            coordinate_shader_start_in_final_thread_section: cs.base.single_seg,
            vertex_shader_start_in_final_thread_section: vs.base.single_seg,
            fragment_shader_start_in_final_thread_section: fs.base.single_seg,
            vertex_id_read_by_coordinate_shader: cs.uses_vid,
            base_instance_id_read_by_coordinate_shader: cs.uses_biid,
            instance_id_read_by_coordinate_shader: cs.uses_iid,
            vertex_id_read_by_vertex_shader: vs.uses_vid,
            base_instance_id_read_by_vertex_shader: vs.uses_biid,
            instance_id_read_by_vertex_shader: vs.uses_iid,
            ..Default::default()
        }
        .pack(&mut shader_state_record);
    } else {
        GLShaderStateRecord {
            enable_clipping: true,
            point_size_in_shaded_vertex_data,
            fragment_shader_does_z_writes: fs.writes_z,
            turn_off_early_z_test: fs.disable_ez,
            fragment_shader_uses_real_pixel_centre_w_in_addition_to_centroid_w2: fs.uses_center_w,
            enable_sample_rate_shading: false,
            any_shader_reads_hardware_written_primitive_id: false,
            do_scoreboard_wait_on_first_thread_switch: fs.lock_scoreboard_on_first_thrsw,
            disable_implicit_point_line_varyings: !fs.uses_implicit_point_line_varyings,
            number_of_varyings_in_fragment_shader: fs.num_inputs,
            coordinate_shader_input_vpm_segment_size: cs.vpm_input_size,
            vertex_shader_input_vpm_segment_size: vs.vpm_input_size,
            coordinate_shader_output_vpm_segment_size: cs.vpm_output_size,
            vertex_shader_output_vpm_segment_size: vs.vpm_output_size,
            min_coord_shader_input_segments_required_in_play: vpm_cfg_bin.a_s,
            min_vertex_shader_input_segments_required_in_play: vpm_cfg.a_s,
            min_coord_shader_output_segments_required_in_play_in_addition_to_vcm_cache_size: vpm_cfg_bin.v_e,
            min_vertex_shader_output_segments_required_in_play_in_addition_to_vcm_cache_size: vpm_cfg.v_e,
            coordinate_shader_4_way_threadable: cs.base.threads == 4,
            vertex_shader_4_way_threadable: vs.base.threads == 4,
            fragment_shader_4_way_threadable: fs.base.threads == 4,
            coordinate_shader_start_in_final_thread_section: cs.base.single_seg,
            vertex_shader_start_in_final_thread_section: vs.base.single_seg,
            fragment_shader_start_in_final_thread_section: fs.base.single_seg,
            vertex_id_read_by_coordinate_shader: cs.uses_vid,
            base_instance_id_read_by_coordinate_shader: cs.uses_biid,
            instance_id_read_by_coordinate_shader: cs.uses_iid,
            vertex_id_read_by_vertex_shader: vs.uses_vid,
            base_instance_id_read_by_vertex_shader: vs.uses_biid,
            instance_id_read_by_vertex_shader: vs.uses_iid,
            ..Default::default()
        }
        .pack(&mut shader_state_record);
    }

    let mut vcm_cache_size = [0u8; VCMCacheSize::LENGTH];
    VCMCacheSize {
        number_of_16_vertex_batches_for_binning: vpm_cfg_bin.v_c,
        number_of_16_vertex_batches_for_rendering: vpm_cfg.v_c,
    }
    .pack(&mut vcm_cache_size);

    Packed {
        shader_state_record,
        vcm_cache_size,
        cfg_bits,
        vpm_cfg_bin,
        vpm_cfg,
        color_write_masks,
        sample_mask: (1 << MAX_SAMPLES) - 1,
        va_count: 0,
    }
}

// V3DV_CMD_DIRTY_*: the command buffer state a draw re-emits.
const DIRTY_PIPELINE: u32 = 1 << 1;
const DIRTY_VERTEX_BUFFER: u32 = 1 << 2;
const DIRTY_INDEX_BUFFER: u32 = 1 << 3;
const DIRTY_DESCRIPTOR_SETS: u32 = 1 << 4;
const DIRTY_PUSH_CONSTANTS: u32 = 1 << 5;
const DIRTY_PUSH_CONSTANTS_UBO: u32 = 1 << 6;
const DIRTY_OCCLUSION_QUERY: u32 = 1 << 7;
const DIRTY_VIEW_INDEX: u32 = 1 << 8;
const DIRTY_DRAW_ID: u32 = 1 << 9;

// The MESA_VK_DYNAMIC_* states whose dirty bit decides what a draw emits
// here. CFG stands for the six CFG_BITS clears together (cull mode, front
// face, stencil test enable, depth bounds test enable, depth bias enable,
// rasterizer discard enable).
const DYN_VP_VIEWPORTS: u32 = 1 << 0;
const DYN_VP_SCISSORS: u32 = 1 << 1;
const DYN_CFG: u32 = 1 << 2;
const DYN_RS_LINE_WIDTH: u32 = 1 << 3;
const DYN_CB_BLEND_CONSTANTS: u32 = 1 << 4;
const DYN_CB_COLOR_WRITE_ENABLES: u32 = 1 << 5;

// VkShaderStageFlagBits.
const STAGE_VERTEX: u32 = 0x1;
const STAGE_FRAGMENT: u32 = 0x10;

/// `struct v3dv_cmd_buffer_state` and the job, as far as this model needs
/// them.
struct Recorder<'p, 'a> {
    pass: &'p Pass<'a>,
    packed: Vec<Packed>,
    alloc: &'p mut dyn BoAlloc,
    bcl: Cl,
    rcl: Cl,
    indirect: Cl,
    tiling: FrameTiling,
    tile_aligned_render_area: bool,
    dirty: u32,
    dyn_dirty: u32,
    dirty_push_constants_stages: u32,
    dirty_descriptor_stages: u32,
    push_constants_data: [u32; PUSH_WORDS],
    pipeline: Option<usize>,
    /// `state.uniforms`: the streams' addresses for the shader record.
    uniforms_fs: u32,
    uniforms_vs: u32,
    uniforms_vs_bin: u32,
    draw_count: u32,
    tmu_dirty_rcl: bool,
    tile_alloc: BoRef,
    tile_state: BoRef,
}

/// Records the pass as one job (`vkCmdBeginRenderPass`, per draw
/// `vkCmdBindPipeline`, `vkCmdPushConstants` and `vkCmdDraw`, then
/// `vkCmdEndRenderPass`). `fill(draw, stage, words)` writes the uniform
/// stream of `stage` for draw `draw` (`v3dv_write_uniforms`), at the moment
/// v3dv writes it; `words` has the program's uniform count.
pub fn record(pass: &Pass, alloc: &mut dyn BoAlloc, fill: &mut dyn FnMut(usize, Stage, &mut [u32]))
              -> Result<Job, OutOfMemory> {
    let packed = pass.pipelines.iter().map(|p| pack_pipeline(p, pass.rev)).collect();
    let mut r = Recorder {
        pass,
        packed,
        alloc,
        bcl: Cl::new(BoKind::Bcl),
        rcl: Cl::new(BoKind::Rcl),
        indirect: Cl::new(BoKind::Indirect),
        tiling: FrameTiling::default(),
        tile_aligned_render_area: false,
        dirty: 0,
        dyn_dirty: 0,
        dirty_push_constants_stages: 0,
        dirty_descriptor_stages: 0,
        push_constants_data: [0; PUSH_WORDS],
        pipeline: None,
        uniforms_fs: 0,
        uniforms_vs: 0,
        uniforms_vs_bin: 0,
        draw_count: 0,
        tmu_dirty_rcl: false,
        tile_alloc: BoRef::default(),
        tile_state: BoRef::default(),
    };
    r.subpass_start()?;
    for (i, d) in pass.draws.iter().enumerate() {
        r.bind_graphics_pipeline(d.pipeline);
        r.push_constants(&d.push);
        r.draw(i, d.vertex_count, fill)?;
    }
    r.finish_job()?;
    let submit = SubmitCl {
        bcl_start: r.bcl.bos[0].va,
        bcl_end: r.bcl.address(),
        rcl_start: r.rcl.bo_va(),
        rcl_end: r.rcl.address(),
        qma: r.tile_alloc.va,
        qms: r.tile_alloc.size,
        qts: r.tile_state.va,
        flush_cache: r.tmu_dirty_rcl,
    };
    Ok(Job { bcl: r.bcl.bos, rcl: r.rcl.bos, indirect: r.indirect.bos, tile_alloc: r.tile_alloc,
             tile_state: r.tile_state, submit })
}

impl Recorder<'_, '_> {
    /// `v3dv_cmd_buffer_subpass_start` for the first subpass:
    /// `cmd_buffer_subpass_create_job` (`v3dv_job_init`, which flags all
    /// state dirty, and `v3dv_job_start_frame`), then
    /// `cmd_buffer_update_tile_alignment`. Double-buffer stays off (it
    /// needs V3D_DEBUG=double_buffer); no clears (loadOp LOAD).
    fn subpass_start(&mut self) -> Result<(), OutOfMemory> {
        self.dirty = !0;
        self.dirty_descriptor_stages = !0;
        self.dyn_dirty = !0;

        // v3dX(framebuffer_compute_internal_bpp_msaa): one 32 bpp target.
        let max_internal_bpp = RT_INTERNAL_BPP;
        let total_color_bpp = 4 * internal_bpp_words(RT_INTERNAL_BPP);
        let ra = self.pass.render_area;
        self.start_frame(ra.x + ra.width, ra.y + ra.height, 1, 1, max_internal_bpp, total_color_bpp, false)?;
        self.tile_aligned_render_area = self.area_is_tile_aligned();
        Ok(())
    }

    /// `v3dv_job_start_frame`.
    #[allow(clippy::too_many_arguments)]
    fn start_frame(&mut self, width: u32, height: u32, layers: u32, render_target_count: u32,
                   max_internal_bpp: u32, total_color_bpp: u32, msaa: bool) -> Result<(), OutOfMemory> {
        self.tiling = compute_frame_tiling(width, height, layers, render_target_count, max_internal_bpp,
            total_color_bpp, msaa);
        self.bcl.ensure_space_with_branch(256, self.alloc)?;
        self.emit_binning_prolog(self.tiling.layers);
        Ok(())
    }

    /// `v3dX(job_emit_binning_prolog)`.
    fn emit_binning_prolog(&mut self, layers: u32) {
        let t = self.tiling;
        emit!(self.bcl, NumberOfLayers { number_of_layers: layers });
        emit!(self.bcl, TileBinningModeCfg {
            width_in_pixels: t.width,
            height_in_pixels: t.height,
            tile_allocation_initial_block_size: TILE_ALLOC_INITIAL_BLOCK_SIZE_ENUM,
            tile_allocation_block_size: TILE_ALLOC_OVERFLOW_BLOCK_SIZE_ENUM,
            log2_tile_width: log2_tile_size(t.tile_width),
            log2_tile_height: log2_tile_size(t.tile_height),
        });
        emit!(self.bcl, FlushVCDCache {});
        emit!(self.bcl, StartTileBinning {});
    }

    /// `v3dv_subpass_area_is_tile_aligned` with the granularity of
    /// `get_granularity` (one R32_UINT target) and a framebuffer with edge
    /// padding (`v3dv_CreateFramebuffer`).
    fn area_is_tile_aligned(&self) -> bool {
        let (gw, gh) = choose_tile_size(4 * internal_bpp_words(RT_INTERNAL_BPP), false);
        let a = self.pass.render_area;
        let has_edge_padding = true;
        let fb = self.pass.image;
        a.x.is_multiple_of(gw) && a.y.is_multiple_of(gh)
            && (a.width.is_multiple_of(gw) || (has_edge_padding && a.x + a.width >= fb.width))
            && (a.height.is_multiple_of(gh) || (has_edge_padding && a.y + a.height >= fb.height))
    }

    /// `bind_graphics_pipeline`.
    fn bind_graphics_pipeline(&mut self, pipeline: usize) {
        if self.pipeline == Some(pipeline) {
            return;
        }
        self.pipeline = Some(pipeline);
        self.dirty |= DIRTY_PIPELINE;
    }

    /// `v3dv_CmdPushConstants` of all 128 bytes for the vertex and fragment
    /// stages: dirty only when the words change.
    fn push_constants(&mut self, push: &[u32; PUSH_WORDS]) {
        if self.push_constants_data == *push {
            return;
        }
        self.push_constants_data = *push;
        self.dirty |= DIRTY_PUSH_CONSTANTS | DIRTY_PUSH_CONSTANTS_UBO;
        self.dirty_push_constants_stages |= STAGE_VERTEX | STAGE_FRAGMENT;
    }

    fn pipe(&self) -> &Pipeline {
        &self.pass.pipelines[self.pipeline.expect("v3d_draw: a draw without a pipeline")]
    }

    fn packed(&self) -> &Packed {
        &self.packed[self.pipeline.expect("v3d_draw: a draw without a pipeline")]
    }

    /// `cmd_buffer_draw` without multiview: view index 0, then
    /// `v3dv_cmd_buffer_emit_pre_draw` and `v3dX(cmd_buffer_emit_draw)`.
    /// `vkCmdDraw` drops a draw without vertices.
    fn draw(&mut self, index: usize, vertex_count: u32, fill: &mut dyn FnMut(usize, Stage, &mut [u32]))
            -> Result<(), OutOfMemory> {
        if vertex_count == 0 {
            return Ok(());
        }
        self.emit_pre_draw(index, fill)?;
        self.emit_draw(vertex_count)
    }

    /// `v3dv_cmd_buffer_emit_pre_draw`. Of the state it may emit, these
    /// emit nothing in this model and are left out: the index buffer (none
    /// bound; its dirty bit is cleared), stencil (no test), depth bias and
    /// depth bounds (disabled), the default point size (V3D 7.1, no point
    /// lists); the MSAA restart, the always-flush split, the binning sync
    /// and the double-buffer score do not apply.
    fn emit_pre_draw(&mut self, index: usize, fill: &mut dyn FnMut(usize, Stage, &mut [u32]))
                     -> Result<(), OutOfMemory> {
        self.draw_count += 1;

        let dirty_uniform_state = self.update_gfx_uniform_state(index, fill)?;

        if dirty_uniform_state || self.dirty & DIRTY_VERTEX_BUFFER != 0 {
            self.emit_gl_shader_state()?;
        }
        if self.dirty & DIRTY_PIPELINE != 0 || self.dyn_dirty & DYN_CFG != 0 {
            self.emit_configuration_bits()?;
        }
        if self.dirty & DIRTY_PIPELINE != 0 {
            self.emit_varyings_state()?;
        }
        if self.dyn_dirty & (DYN_VP_SCISSORS | DYN_VP_VIEWPORTS) != 0 {
            self.emit_scissor()?;
        }
        if self.dyn_dirty & DYN_VP_VIEWPORTS != 0 {
            self.emit_viewport()?;
        }
        if self.dirty & DIRTY_INDEX_BUFFER != 0 {
            self.dirty &= !DIRTY_INDEX_BUFFER;
        }
        if self.dirty & DIRTY_PIPELINE != 0 || self.dyn_dirty & DYN_CB_BLEND_CONSTANTS != 0 {
            self.emit_blend()?;
        }
        if self.dirty & DIRTY_OCCLUSION_QUERY != 0 {
            self.emit_occlusion_query()?;
        }
        if self.dyn_dirty & DYN_RS_LINE_WIDTH != 0 {
            self.emit_line_width()?;
        }
        if self.dirty & DIRTY_PIPELINE != 0 {
            self.emit_sample_state()?;
        }
        if self.dirty & DIRTY_PIPELINE != 0 || self.dyn_dirty & DYN_CB_COLOR_WRITE_ENABLES != 0 {
            self.emit_color_write_mask()?;
        }
        self.dirty &= !DIRTY_PIPELINE;
        Ok(())
    }

    /// `update_gfx_uniform_state` without geometry shaders, descriptors or
    /// software blending: the fragment stream, then the vertex and
    /// coordinate streams, each rewritten when its inputs changed.
    fn update_gfx_uniform_state(&mut self, index: usize, fill: &mut dyn FnMut(usize, Stage, &mut [u32]))
                                -> Result<bool, OutOfMemory> {
        let dirty = self.dirty;
        let dirty_uniform_state = dirty
            & (DIRTY_PIPELINE | DIRTY_PUSH_CONSTANTS | DIRTY_DESCRIPTOR_SETS | DIRTY_VIEW_INDEX | DIRTY_DRAW_ID)
            != 0
            || self.dyn_dirty & DYN_VP_VIEWPORTS != 0;
        if !dirty_uniform_state {
            return Ok(false);
        }
        let has_new_pipeline = dirty & DIRTY_PIPELINE != 0;
        let has_new_viewport = self.dyn_dirty & DYN_VP_VIEWPORTS != 0;
        let has_new_push_constants = dirty & DIRTY_PUSH_CONSTANTS != 0;
        let has_new_descriptors = dirty & DIRTY_DESCRIPTOR_SETS != 0;
        let has_new_view_index = dirty & DIRTY_VIEW_INDEX != 0;
        let has_new_draw_id = dirty & DIRTY_DRAW_ID != 0;

        let has_new_descriptors_fs = has_new_descriptors && self.dirty_descriptor_stages & STAGE_FRAGMENT != 0;
        let has_new_push_constants_fs =
            has_new_push_constants && self.dirty_push_constants_stages & STAGE_FRAGMENT != 0;
        if has_new_pipeline || has_new_view_index || has_new_push_constants_fs || has_new_descriptors_fs {
            let count = self.pipe().fs.prog.base.uniform_count;
            self.uniforms_fs = self.write_uniforms(index, Stage::Fragment, count, fill)?;
        }

        let has_new_descriptors_vs = has_new_descriptors && self.dirty_descriptor_stages & STAGE_VERTEX != 0;
        let has_new_push_constants_vs =
            has_new_push_constants && self.dirty_push_constants_stages & STAGE_VERTEX != 0;
        if has_new_viewport || has_new_view_index || has_new_draw_id || has_new_pipeline
            || has_new_push_constants_vs || has_new_descriptors_vs
        {
            let count = self.pipe().vs.prog.base.uniform_count;
            self.uniforms_vs = self.write_uniforms(index, Stage::Vertex, count, fill)?;
            let count = self.pipe().cs.prog.base.uniform_count;
            self.uniforms_vs_bin = self.write_uniforms(index, Stage::Coordinate, count, fill)?;
        }
        self.dirty &= !(DIRTY_VIEW_INDEX | DIRTY_DRAW_ID);
        Ok(true)
    }

    /// `v3dv_write_uniforms`: room for one word more than the stream (the
    /// QPU prefetches the next uniform), the stream's address returned.
    fn write_uniforms(&mut self, index: usize, stage: Stage, count: u32,
                      fill: &mut dyn FnMut(usize, Stage, &mut [u32])) -> Result<u32, OutOfMemory> {
        self.indirect.ensure_space((count + 1) * 4, 4, self.alloc)?;
        let addr = self.indirect.address();
        let mut words = vec![0u32; count as usize];
        fill(index, stage, &mut words);
        for w in words {
            self.indirect.put(4, |b| b.copy_from_slice(&w.to_le_bytes()));
        }
        Ok(addr)
    }

    /// `v3dX(cmd_buffer_emit_gl_shader_state)` without a geometry shader and
    /// without vertex attributes (one dummy attribute, GFXH-930).
    fn emit_gl_shader_state(&mut self) -> Result<(), OutOfMemory> {
        let p = *self.pipe();
        self.tmu_dirty_rcl |= p.cs.prog.base.tmu_dirty_rcl;
        self.tmu_dirty_rcl |= p.vs.prog.base.tmu_dirty_rcl;
        self.tmu_dirty_rcl |= p.fs.prog.base.tmu_dirty_rcl;

        let va_count = self.packed().va_count;
        assert_eq!(va_count, 0, "v3d_draw: vertex attributes");
        let num_elements_to_emit = va_count.max(1);
        let draw_index = has_draw_index(self.pass.rev);
        let shader_state_record_length = if draw_index {
            GLShaderStateRecordDrawIndex::LENGTH
        } else {
            GLShaderStateRecord::LENGTH
        } as u32;
        let shader_rec_offset = self.indirect.ensure_space(
            shader_state_record_length + num_elements_to_emit * GLShaderStateAttributeRecord::LENGTH as u32, 32,
            self.alloc)?;

        let (vpm_bin_as, vpm_as) = (self.packed().vpm_cfg_bin.a_s, self.packed().vpm_cfg.a_s);
        let any_reads_pid = p.fs.prog.uses_pid;
        let insert_pid = p.fs.prog.uses_pid;
        let record = self.packed[self.pipeline.unwrap()].shader_state_record.clone();
        let (fs_u, vs_u, cs_u) = (self.uniforms_fs, self.uniforms_vs, self.uniforms_vs_bin);
        if draw_index {
            self.indirect.put_with_prepacked(&record, |b| {
                GLShaderStateRecordDrawIndex {
                    min_coord_shader_input_segments_required_in_play: vpm_bin_as,
                    min_vertex_shader_input_segments_required_in_play: vpm_as,
                    coordinate_shader_code_address: p.cs.code,
                    vertex_shader_code_address: p.vs.code,
                    fragment_shader_code_address: p.fs.code,
                    coordinate_shader_uniforms_address: cs_u,
                    vertex_shader_uniforms_address: vs_u,
                    fragment_shader_uniforms_address: fs_u,
                    any_shader_reads_hardware_written_primitive_id: any_reads_pid,
                    insert_primitive_id_as_first_varying_to_fragment_shader: insert_pid,
                    ..Default::default()
                }
                .pack(b)
            });
        } else {
            self.indirect.put_with_prepacked(&record, |b| {
                GLShaderStateRecord {
                    min_coord_shader_input_segments_required_in_play: vpm_bin_as,
                    min_vertex_shader_input_segments_required_in_play: vpm_as,
                    coordinate_shader_code_address: p.cs.code,
                    vertex_shader_code_address: p.vs.code,
                    fragment_shader_code_address: p.fs.code,
                    coordinate_shader_uniforms_address: cs_u,
                    vertex_shader_uniforms_address: vs_u,
                    fragment_shader_uniforms_address: fs_u,
                    any_shader_reads_hardware_written_primitive_id: any_reads_pid,
                    insert_primitive_id_as_first_varying_to_fragment_shader: insert_pid,
                    ..Default::default()
                }
                .pack(b)
            });
        }

        // GFXH-930: at least one attribute read by CS and VS; the data at a
        // valid address is never used.
        let indirect_va = self.indirect.bo_va();
        emit!(self.indirect, GLShaderStateAttributeRecord {
            address: indirect_va,
            type_: ATTRIBUTE_FLOAT,
            stride: 0,
            vec_size: 1,
            number_of_values_read_by_coordinate_shader: 1,
            number_of_values_read_by_vertex_shader: 1,
            ..Default::default()
        });

        if self.dirty & DIRTY_PIPELINE != 0 {
            self.bcl.ensure_space_with_branch(VCMCacheSize::LENGTH as u32, self.alloc)?;
            let vcm = self.packed().vcm_cache_size;
            self.bcl.put(vcm.len(), |b| b.copy_from_slice(&vcm));
        }

        self.bcl.ensure_space_with_branch(GLShaderState::LENGTH as u32, self.alloc)?;
        emit!(self.bcl, GLShaderState {
            address: indirect_va + shader_rec_offset,
            number_of_attribute_arrays: num_elements_to_emit,
        });

        self.dirty &= !(DIRTY_VERTEX_BUFFER | DIRTY_DESCRIPTOR_SETS | DIRTY_PUSH_CONSTANTS);
        self.dirty_descriptor_stages &= !(STAGE_VERTEX | STAGE_FRAGMENT);
        self.dirty_push_constants_stages &= !(STAGE_VERTEX | STAGE_FRAGMENT);
        Ok(())
    }

    /// `v3dX(cmd_buffer_emit_configuration_bits)`: no depth or stencil
    /// attachment, cull none, counter-clockwise front faces, no depth bias.
    fn emit_configuration_bits(&mut self) -> Result<(), OutOfMemory> {
        self.bcl.ensure_space_with_branch(CfgBits::LENGTH as u32, self.alloc)?;
        let prepacked = self.packed().cfg_bits;
        self.bcl.put_with_prepacked(&prepacked, |b| {
            CfgBits {
                depth_test_function: compare_function::ALWAYS,
                stencil_enable: false,
                z_updates_enable: false,
                enable_forward_facing_primitive: true,
                enable_reverse_facing_primitive: true,
                clockwise_primitives: true,
                depth_bounds_test_enable: false,
                enable_depth_offset: false,
                ..Default::default()
            }
            .pack(b)
        });
        self.dyn_dirty &= !DYN_CFG;
        Ok(())
    }

    /// `v3dX(cmd_buffer_emit_varyings_state)` with `emit_varying_flags` and
    /// its three packet emitters.
    fn emit_varyings_state(&mut self) -> Result<(), OutOfMemory> {
        let fs = self.pipe().fs.prog;
        if !self.emit_varying_flags(&fs.flat_shade_flags, |cl, off, v, lower, higher| {
            emit!(cl, FlatShadeFlags {
                varying_offset_v0: off,
                flat_shade_flags_for_varyings_v024: v,
                action_for_flat_shade_flags_of_lower_numbered_varyings: lower,
                action_for_flat_shade_flags_of_higher_numbered_varyings: higher,
            })
        }, FlatShadeFlags::LENGTH)?
        {
            self.bcl.ensure_space_with_branch(ZeroAllFlatShadeFlags::LENGTH as u32, self.alloc)?;
            emit!(self.bcl, ZeroAllFlatShadeFlags {});
        }
        if !self.emit_varying_flags(&fs.noperspective_flags, |cl, off, v, lower, higher| {
            emit!(cl, NonPerspectiveFlags {
                varying_offset_v0: off,
                non_perspective_flags_for_varyings_v024: v,
                action_for_non_perspective_flags_of_lower_numbered_varyings: lower,
                action_for_non_perspective_flags_of_higher_numbered_varyings: higher,
            })
        }, NonPerspectiveFlags::LENGTH)?
        {
            self.bcl.ensure_space_with_branch(ZeroAllNonPerspectiveFlags::LENGTH as u32, self.alloc)?;
            emit!(self.bcl, ZeroAllNonPerspectiveFlags {});
        }
        if !self.emit_varying_flags(&fs.centroid_flags, |cl, off, v, lower, higher| {
            emit!(cl, CentroidFlags {
                varying_offset_v0: off,
                centroid_flags_for_varyings_v024: v,
                action_for_centroid_flags_of_lower_numbered_varyings: lower,
                action_for_centroid_flags_of_higher_numbered_varyings: higher,
            })
        }, CentroidFlags::LENGTH)?
        {
            self.bcl.ensure_space_with_branch(ZeroAllCentroidFlags::LENGTH as u32, self.alloc)?;
            emit!(self.bcl, ZeroAllCentroidFlags {});
        }
        Ok(())
    }

    /// `emit_varying_flags`: true when any word had flags.
    fn emit_varying_flags(&mut self, flags: &[u32; VARYING_FLAG_WORDS],
                          emit: impl Fn(&mut Cl, u32, u32, u32, u32), len: usize) -> Result<bool, OutOfMemory> {
        use varying_flags_action::{UNCHANGED, ZEROED};
        let mut emitted_any = false;
        for (i, &f) in flags.iter().enumerate() {
            if f == 0 {
                continue;
            }
            self.bcl.ensure_space_with_branch(len as u32, self.alloc)?;
            if emitted_any {
                emit(&mut self.bcl, i as u32, f, UNCHANGED, UNCHANGED);
            } else if i == 0 {
                emit(&mut self.bcl, i as u32, f, UNCHANGED, ZEROED);
            } else {
                emit(&mut self.bcl, i as u32, f, ZEROED, ZEROED);
            }
            emitted_any = true;
        }
        Ok(emitted_any)
    }

    /// `emit_scissor` and `v3dX(job_emit_clip_window)`: the viewport clipped
    /// to the render area and the scissor.
    fn emit_scissor(&mut self) -> Result<(), OutOfMemory> {
        let (scale, translate) = viewport_compute_xform(&self.pass.viewport);
        assert!(scale[0] >= 0.0);
        let vp_minx = translate[0] - scale[0];
        let vp_maxx = translate[0] + scale[0];
        let vp_miny = translate[1] - scale[1].abs();
        let vp_maxy = translate[1] + scale[1].abs();
        let ra = self.pass.render_area;
        let mut minx = vp_minx.max(ra.x as f32) as u32;
        let mut miny = vp_miny.max(ra.y as f32) as u32;
        let mut maxx = vp_maxx.min((ra.x + ra.width) as f32) as u32;
        let mut maxy = vp_maxy.min((ra.y + ra.height) as f32) as u32;
        let sc = self.pass.scissor;
        minx = minx.max(sc.x);
        miny = miny.max(sc.y);
        maxx = maxx.min(sc.x + sc.width);
        maxy = maxy.min(sc.y + sc.height);
        if minx > maxx {
            maxx = minx;
        }
        if miny > maxy {
            maxy = miny;
        }
        self.bcl.ensure_space_with_branch(ClipWindow::LENGTH as u32, self.alloc)?;
        emit!(self.bcl, ClipWindow {
            clip_window_left_pixel_coordinate: minx,
            clip_window_bottom_pixel_coordinate: miny,
            clip_window_width_in_pixels: maxx - minx,
            clip_window_height_in_pixels: maxy - miny,
        });
        self.dyn_dirty &= !DYN_VP_SCISSORS;
        Ok(())
    }

    /// `v3dX(cmd_buffer_emit_viewport)` with
    /// `v3dv_cmd_buffer_state_get_viewport_z_xform` (depth range 0..1, not
    /// negative-one-to-one).
    fn emit_viewport(&mut self) -> Result<(), OutOfMemory> {
        let (vpscale, vptranslate) = viewport_compute_xform(&self.pass.viewport);
        let required = ClipperXYScaling::LENGTH + ClipperZScaleAndOffset::LENGTH
            + ClipperZMinMaxClippingPlanes::LENGTH + ViewportOffset::LENGTH;
        self.bcl.ensure_space_with_branch(required as u32, self.alloc)?;
        emit!(self.bcl, ClipperXYScaling {
            viewport_half_width_in_1_64th_of_pixel: vpscale[0] * 64.0,
            viewport_half_height_in_1_64th_of_pixel: vpscale[1] * 64.0,
        });
        let (translate_z, scale_z) = (vptranslate[2], vpscale[2]);
        // A Z scale this small defeats guardband clipping.
        if scale_z.abs() < 0.01 {
            emit!(self.bcl, ClipperZScaleAndOffsetNoGuardband {
                viewport_z_offset_zc_to_zs: translate_z,
                viewport_z_scale_zc_to_zs: scale_z,
            });
        } else {
            emit!(self.bcl, ClipperZScaleAndOffset {
                viewport_z_offset_zc_to_zs: translate_z,
                viewport_z_scale_zc_to_zs: scale_z,
            });
        }
        let z1 = translate_z;
        let z2 = translate_z + scale_z;
        emit!(self.bcl, ClipperZMinMaxClippingPlanes { minimum_zw: z1.min(z2), maximum_zw: z1.max(z2) });

        let mut vp_fine_x = vptranslate[0];
        let mut vp_fine_y = vptranslate[1];
        let mut vp_coarse_x = 0i32;
        let mut vp_coarse_y = 0i32;
        // The fine coordinates must be unsigned; the coarse ones take the
        // 64-pixel blocks below zero.
        if vp_fine_x < 0.0 {
            let blocks_64 = ((vp_fine_x.abs() + 64.0 - 1.0) / 64.0) as i32;
            vp_fine_x += 64.0 * blocks_64 as f32;
            vp_coarse_x -= blocks_64;
        }
        if vp_fine_y < 0.0 {
            let blocks_64 = ((vp_fine_y.abs() + 64.0 - 1.0) / 64.0) as i32;
            vp_fine_y += 64.0 * blocks_64 as f32;
            vp_coarse_y -= blocks_64;
        }
        emit!(self.bcl, ViewportOffset {
            fine_x: vp_fine_x,
            fine_y: vp_fine_y,
            coarse_x: vp_coarse_x,
            coarse_y: vp_coarse_y,
        });
        self.dyn_dirty &= !DYN_VP_VIEWPORTS;
        Ok(())
    }

    /// `v3dX(cmd_buffer_emit_blend)` without blending and without blend
    /// constants: the space check only.
    fn emit_blend(&mut self) -> Result<(), OutOfMemory> {
        let size = BlendEnables::LENGTH + BlendConstantColor::LENGTH + BlendCfg::LENGTH * MAX_RENDER_TARGETS as usize;
        self.bcl.ensure_space_with_branch(size as u32, self.alloc)?;
        self.dyn_dirty &= !DYN_CB_BLEND_CONSTANTS;
        Ok(())
    }

    /// `v3dX(cmd_buffer_emit_occlusion_query)` without an active query.
    fn emit_occlusion_query(&mut self) -> Result<(), OutOfMemory> {
        self.bcl.ensure_space_with_branch(OcclusionQueryCounter::LENGTH as u32, self.alloc)?;
        emit!(self.bcl, OcclusionQueryCounter { address: 0 });
        self.dirty &= !DIRTY_OCCLUSION_QUERY;
        Ok(())
    }

    /// `v3dX(cmd_buffer_emit_line_width)`: width 1, no line smoothing
    /// (`v3dv_get_aa_line_width`).
    fn emit_line_width(&mut self) -> Result<(), OutOfMemory> {
        self.bcl.ensure_space_with_branch(LineWidth::LENGTH as u32, self.alloc)?;
        emit!(self.bcl, LineWidth { line_width: 1.0 });
        self.dyn_dirty &= !DYN_RS_LINE_WIDTH;
        Ok(())
    }

    /// `v3dX(cmd_buffer_emit_sample_state)`.
    fn emit_sample_state(&mut self) -> Result<(), OutOfMemory> {
        self.bcl.ensure_space_with_branch(SampleState::LENGTH as u32, self.alloc)?;
        let mask = self.packed().sample_mask;
        emit!(self.bcl, SampleState { coverage: 1.0, mask });
        Ok(())
    }

    /// `v3dX(cmd_buffer_emit_color_write_mask)`, with every attachment's
    /// writes enabled (`pipeline_init_dynamic_state`).
    fn emit_color_write_mask(&mut self) -> Result<(), OutOfMemory> {
        self.bcl.ensure_space_with_branch(ColorWriteMasks::LENGTH as u32, self.alloc)?;
        let color_write_enable = 0xfu32;
        let mask = !color_write_enable | self.packed().color_write_masks;
        emit!(self.bcl, ColorWriteMasks { mask });
        self.dyn_dirty &= !DYN_CB_COLOR_WRITE_ENABLES;
        Ok(())
    }

    /// `v3dX(cmd_buffer_emit_draw)` for one instance from vertex 0.
    fn emit_draw(&mut self, vertex_count: u32) -> Result<(), OutOfMemory> {
        self.bcl.ensure_space_with_branch(VertexArrayPrims::LENGTH as u32, self.alloc)?;
        emit!(self.bcl, VertexArrayPrims {
            mode: primitive::TRIANGLE_STRIP,
            length: vertex_count,
            index_of_first_vertex: 0,
        });
        Ok(())
    }

    /// `v3dv_cmd_buffer_finish_job` at `vkCmdEndRenderPass`
    /// (`cmd_buffer_end_render_pass_frame`): tile memory, the render list,
    /// the binning flush.
    fn finish_job(&mut self) -> Result<(), OutOfMemory> {
        self.allocate_tile_state()?;
        self.emit_render_pass_rcl()?;
        self.emit_binning_flush()
    }

    /// `v3dv_job_allocate_tile_state` with `v3dv_bo_alloc`'s page
    /// rounding.
    fn allocate_tile_state(&mut self) -> Result<(), OutOfMemory> {
        let t = self.tiling;
        let (alloc_size, state_size) = tile_alloc_sizes(t.layers, t.draw_tiles_x, t.draw_tiles_y, self.draw_count);
        let size = align(alloc_size, PAGE_SIZE);
        self.tile_alloc = BoRef { va: self.alloc.alloc(BoKind::TileAlloc, size).ok_or(OutOfMemory)?, size };
        let size = align(state_size, PAGE_SIZE);
        self.tile_state = BoRef { va: self.alloc.alloc(BoKind::TileState, size).ok_or(OutOfMemory)?, size };
        Ok(())
    }

    /// `v3dX(job_emit_binning_flush)`.
    fn emit_binning_flush(&mut self) -> Result<(), OutOfMemory> {
        self.bcl.ensure_space_with_branch(Flush::LENGTH as u32, self.alloc)?;
        emit!(self.bcl, Flush {});
        Ok(())
    }

    /// `v3dv_cmd_buffer_check_needs_load` for the colour attachment: the
    /// job runs the attachment's only subpass from its start, so loadOp
    /// LOAD decides (and a store outside a tile-aligned area would too).
    fn color_needs_load(&self) -> bool {
        let load_op_load = true;
        (!self.tile_aligned_render_area && self.color_needs_store()) || load_op_load
    }

    /// `v3dv_cmd_buffer_check_needs_store` for the colour attachment in the
    /// last job of its last subpass: storeOp STORE decides.
    fn color_needs_store(&self) -> bool {
        true
    }

    /// `v3dX(cmd_buffer_emit_render_pass_rcl)` for one colour target and no
    /// depth/stencil attachment.
    fn emit_render_pass_rcl(&mut self) -> Result<(), OutOfMemory> {
        let t = self.tiling;
        let fb_layers = t.layers;
        self.rcl.ensure_space_with_branch(200 + fb_layers.max(1) * 256 * SupertileCoordinates::LENGTH as u32,
            self.alloc)?;

        let fb = self.pass.image;
        let color_count = 1;
        emit!(self.rcl, TileRenderingModeCfgCommon {
            image_width_pixels: fb.width,
            image_height_pixels: fb.height,
            number_of_render_targets: color_count,
            multisample_mode_4x: t.msaa,
            double_buffer_in_non_ms_mode: false,
            log2_tile_width: log2_tile_size(t.tile_width),
            log2_tile_height: log2_tile_size(t.tile_height),
            early_z_disable: true,
            ..Default::default()
        });

        // cmd_buffer_render_pass_setup_render_target; one target, so its
        // base address in the tile buffer is 0. No PART2/PART3: 32 bpp.
        let stride = rt_row_row_stride_128_bits(t.tile_width, internal_bpp_words(RT_INTERNAL_BPP));
        emit!(self.rcl, TileRenderingModeCfgRenderTargetPart1 {
            clear_color_low_bits: 0,
            internal_bpp: RT_INTERNAL_BPP,
            internal_type_and_clamping: clamp_for_format_and_type(RT_INTERNAL_TYPE),
            stride,
            base_address: 0,
            render_target_number: 0,
            ..Default::default()
        });

        emit!(self.rcl, TileRenderingModeCfgZSClearValues {
            z_clear_value: 1.0,
            stencil_clear_value: 0,
            ..Default::default()
        });

        // The first branch needs the binning mode's initial block size.
        emit!(self.rcl, TileListInitialBlockSize {
            use_auto_chained_tile_lists: true,
            size_of_first_block_in_chained_tile_lists: TILE_ALLOC_INITIAL_BLOCK_SIZE_ENUM,
        });

        emit!(self.rcl, MulticoreRenderingSupertileCfg {
            number_of_bin_tile_lists: 1,
            total_frame_width_in_tiles: t.draw_tiles_x,
            total_frame_height_in_tiles: t.draw_tiles_y,
            supertile_width_in_tiles: t.supertile_width,
            supertile_height_in_tiles: t.supertile_height,
            total_frame_width_in_supertiles: t.frame_width_in_supertiles,
            total_frame_height_in_supertiles: t.frame_height_in_supertiles,
            ..Default::default()
        });

        // An initial clear of the tile buffers, and GFXH-1742: two dummy
        // stores so the QPUs see the TLB's new internal type and size.
        for i in 0..2 {
            emit!(self.rcl, TileCoordinates { ..Default::default() });
            emit!(self.rcl, EndOfLoads {});
            emit!(self.rcl, StoreTileBufferGeneral { buffer_to_store: NONE, ..Default::default() });
            // v3dv_do_double_initial_tile_clear is false without double-buffer.
            if self.tile_aligned_render_area && i == 0 {
                emit!(self.rcl, ClearRenderTargets {});
            }
            emit!(self.rcl, EndOfTileMarker {});
        }

        emit!(self.rcl, FlushVCDCache {});

        for layer in 0..fb_layers.max(1) {
            self.emit_render_pass_layer_rcl(layer)?;
        }

        emit!(self.rcl, EndOfRendering {});
        Ok(())
    }

    /// `cmd_buffer_emit_render_pass_layer_rcl`.
    fn emit_render_pass_layer_rcl(&mut self, layer: u32) -> Result<(), OutOfMemory> {
        let t = self.tiling;
        let tile_alloc_offset = TILE_ALLOC_INITIAL_BLOCK_SIZE * layer * t.draw_tiles_x * t.draw_tiles_y;
        emit!(self.rcl, MulticoreRenderingTileListSetBase {
            address: self.tile_alloc.va + tile_alloc_offset,
            ..Default::default()
        });

        self.emit_per_tile_rcl(layer)?;

        let supertile_w_in_pixels = t.tile_width * t.supertile_width;
        let supertile_h_in_pixels = t.tile_height * t.supertile_height;
        let ra = self.pass.render_area;
        let min_x_supertile = ra.x / supertile_w_in_pixels;
        let min_y_supertile = ra.y / supertile_h_in_pixels;
        let mut max_render_x = ra.x;
        if ra.width > 0 {
            max_render_x += ra.width - 1;
        }
        let mut max_render_y = ra.y;
        if ra.height > 0 {
            max_render_y += ra.height - 1;
        }
        let max_x_supertile = max_render_x / supertile_w_in_pixels;
        let max_y_supertile = max_render_y / supertile_h_in_pixels;
        for y in min_y_supertile..=max_y_supertile {
            for x in min_x_supertile..=max_x_supertile {
                emit!(self.rcl, SupertileCoordinates { column_number_in_supertiles: x, row_number_in_supertiles: y });
            }
        }
        Ok(())
    }

    /// `cmd_buffer_render_pass_emit_per_tile_rcl`: the generic tile list in
    /// the indirect buffer, which the render list points at.
    fn emit_per_tile_rcl(&mut self, _layer: u32) -> Result<(), OutOfMemory> {
        self.indirect.ensure_space(200, 1, self.alloc)?;
        let tile_list_start = self.indirect.address();

        emit!(self.indirect, TileCoordinatesImplicit {});

        // cmd_buffer_render_pass_emit_loads and
        // cmd_buffer_render_pass_emit_load: the colour target from the
        // raster image.
        let img = self.pass.image;
        if self.color_needs_load() {
            emit!(self.indirect, LoadTileBufferGeneral {
                buffer_to_load: RENDER_TARGET_0,
                address: img.va,
                input_image_format: RT_OUTPUT_FORMAT,
                r_b_swap: false,
                channel_reverse: false,
                memory_format: memory_format::RASTER,
                height_in_ub_or_stride: img.stride,
                decimate_mode: decimate_mode::SAMPLE_0,
                ..Default::default()
            });
        }
        emit!(self.indirect, EndOfLoads {});

        // The binner starts writing tiles assuming triangles.
        emit!(self.indirect, PrimListFormat { primitive_type: LIST_TRIANGLES, ..Default::default() });
        // The PTB assumes 0 here but the hardware does not set it.
        emit!(self.indirect, SetInstanceID { instance_id: 0 });
        emit!(self.indirect, BranchToImplicitTileList { tile_list_set_number: 0 });

        // cmd_buffer_render_pass_emit_stores and
        // cmd_buffer_render_pass_emit_store: no clear (loadOp LOAD).
        if self.color_needs_store() {
            emit!(self.indirect, StoreTileBufferGeneral {
                buffer_to_store: RENDER_TARGET_0,
                address: img.va,
                clear_buffer_being_stored: false,
                output_image_format: RT_OUTPUT_FORMAT,
                r_b_swap: false,
                channel_reverse: false,
                memory_format: memory_format::RASTER,
                height_in_ub_or_stride: img.stride,
                decimate_mode: decimate_mode::SAMPLE_0,
                ..Default::default()
            });
        } else {
            emit!(self.indirect, StoreTileBufferGeneral { buffer_to_store: NONE, ..Default::default() });
        }

        emit!(self.indirect, EndOfTileMarker {});
        emit!(self.indirect, ReturnFromSubList {});

        let end = self.indirect.address();
        emit!(self.rcl, StartAddressOfGenericTileList { start: tile_list_start, end });
        Ok(())
    }
}
