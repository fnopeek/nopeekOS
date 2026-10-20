/*
 * imgdesc: import a guest frame the way radeonsi imports a dma-buf without
 * a modifier (si_texture_from_winsys_buffer), for Renoir, and print what
 * Mesa's common code computed: the surface, the size check radeonsi makes,
 * and the sampled-image descriptor (T#) of a 2D B8G8R8A8_UNORM view at a
 * known address. The module builds the same descriptor itself; its host
 * check holds it against these lines.
 *
 * The steps are radeonsi's, each through the common function it calls:
 *   amdgpu_bo_get_metadata          ac_surface_apply_bo_metadata
 *   si_init_surface                 ac_compute_surface, IMPORTED | SHAREABLE
 *   si_texture_from_winsys_buffer   ac_surface_apply_umd_metadata
 *   si_make_texture_descriptor      ac_build_texture_descriptor
 *   si_set_mutable_tex_desc_fields  ac_set_mutable_tex_desc_fields, epitch
 * The device's radeon_info comes from RADV behind Mesa's amdgpu drm-shim;
 * the ac_* code is linked from libamd_common.a and libaddrlib.a.
 *
 * Each input line is a case:
 *   import <w> <h> <tiling flags> <n> <word0> ... <word n-1>
 *       the BO's tiling flags and UMD metadata, as AMDGPU_GEM_METADATA
 *       returns them.
 * Each case prints "case ..." lines, read by the host check.
 *
 * Usage: imgdesc < cases > out
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "ac_descriptors.h"
#include "ac_gpu_info.h"
#include "ac_surface.h"
#include "drm-uapi/amdgpu_drm.h"
#include "drm-uapi/drm_fourcc.h"
#include "radv_physical_device.h"
#include "sid.h"
#include "util/format/u_format.h"

PUBLIC VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL vk_icdGetInstanceProcAddr(VkInstance instance, const char *pName);

#define GET(inst, name) PFN_##name name = (PFN_##name)vk_icdGetInstanceProcAddr(inst, #name)

/* Where the view's image lies in the oracle's address space. */
#define VA 0x0000800000100000ull

static struct radeon_info info;
static struct ac_addrlib *addrlib;

static void
print_surface(const struct radeon_surf *s)
{
   printf("case surface swizzle %u scanout %u epitch %u surf_pitch %u surf_height %u surf_size %llu "
          "surf_align_log2 %u meta_offset %llu meta_size %u display_dcc_size %u meta_align_log2 %u "
          "pipe_aligned %u rb_aligned %u num_meta_levels %u tile_swizzle %u\n",
          s->u.gfx9.swizzle_mode, !!(s->flags & RADEON_SURF_SCANOUT), s->u.gfx9.epitch, s->u.gfx9.surf_pitch,
          s->u.gfx9.surf_height, (unsigned long long)s->surf_size, s->surf_alignment_log2,
          (unsigned long long)s->meta_offset, s->meta_size, s->u.gfx9.color.display_dcc_size, s->meta_alignment_log2,
          s->u.gfx9.color.dcc.pipe_aligned, s->u.gfx9.color.dcc.rb_aligned, s->num_meta_levels, s->tile_swizzle);
}

/* si_init_surface for a 2D B8G8R8A8_UNORM texture, one level, one sample:
 * amdgpu_surface_init's config from the pipe_resource. */
static int
init_surface(struct radeon_surf *surf, unsigned w, unsigned h, uint64_t flags, enum radeon_surf_mode mode)
{
   struct ac_surf_config config;
   memset(&config, 0, sizeof(config));
   config.info.width = w;
   config.info.height = h;
   config.info.depth = 1;
   config.info.array_size = 1;
   config.info.samples = 0;
   config.info.storage_samples = 0;
   config.info.levels = 1;
   config.info.num_channels = util_format_get_nr_components(PIPE_FORMAT_B8G8R8A8_UNORM);
   config.blk_w = 1;
   config.blk_h = 1;
   config.bpe = 4;
   config.surf_flags = flags;
   config.modifier = surf->modifier;
   config.gfx9.swizzle_mode = surf->u.gfx9.swizzle_mode;
   config.gfx9.dcc_max_compressed_block_size = surf->u.gfx9.color.dcc.max_compressed_block_size;
   config.gfx9.dcc_independent_64B_blocks = surf->u.gfx9.color.dcc.independent_64B_blocks;
   config.gfx9.dcc_independent_128B_blocks = surf->u.gfx9.color.dcc.independent_128B_blocks;
   config.gfx9.display_dcc_pitch_max = surf->u.gfx9.color.display_dcc_pitch_max;
   return ac_compute_surface(addrlib, &info, &config, mode, surf);
}

static void
import_case(unsigned w, unsigned h, uint64_t tiling, unsigned n, const uint32_t *md)
{
   struct radeon_surf surf;
   memset(&surf, 0, sizeof(surf));
   enum radeon_surf_mode mode = RADEON_SURF_MODE_LINEAR_ALIGNED;
   ac_surface_apply_bo_metadata(info.gfx_level, &surf, tiling, &mode);
   uint64_t flags = RADEON_SURF_IMPORTED | RADEON_SURF_SHAREABLE;
   if (surf.flags & RADEON_SURF_SCANOUT)
      flags |= RADEON_SURF_SCANOUT;
   surf.modifier = DRM_FORMAT_MOD_INVALID;
   int r = init_surface(&surf, w, h, flags, mode);
   if (r) {
      printf("case refused surface %d\n", r);
      return;
   }
   uint32_t metadata[64] = {0};
   memcpy(metadata, md, n * 4);
   if (!ac_surface_apply_umd_metadata(&info, &surf, 0, 1, n * 4, metadata)) {
      printf("case refused metadata\n");
      return;
   }
   print_surface(&surf);

   const struct util_format_description *desc = util_format_description(PIPE_FORMAT_B8G8R8A8_UNORM);
   const bool dcc = surf.meta_offset != 0 && surf.num_meta_levels > 0;
   const struct ac_texture_state tex_state = {
      .surf = &surf,
      .format = PIPE_FORMAT_B8G8R8A8_UNORM,
      .img_format = PIPE_FORMAT_B8G8R8A8_UNORM,
      .width = w,
      .height = h,
      .depth = 1,
      .type = V_008F1C_SQ_RSRC_IMG_2D,
      .swizzle = {desc->swizzle[0], desc->swizzle[1], desc->swizzle[2], desc->swizzle[3]},
      .num_samples = 0,
      .num_storage_samples = 0,
      .first_level = 0,
      .last_level = 0,
      .num_levels = 1,
      .first_layer = 0,
      .last_layer = 0,
      .dcc_enabled = dcc,
      .tc_compat_htile_enabled = true,
   };
   uint32_t t[8];
   ac_build_texture_descriptor(&info, &tex_state, t);
   const struct ac_mutable_tex_state m = {
      .surf = &surf,
      .va = VA,
      .gfx6 = {.base_level = 0, .block_width = 1},
      .dcc_enabled = dcc,
   };
   ac_set_mutable_tex_desc_fields(&info, &m, t);
   t[4] &= C_008F20_PITCH;
   t[4] |= S_008F20_PITCH(surf.u.gfx9.epitch);
   printf("case va %llu\n", (unsigned long long)VA);
   printf("case desc");
   for (int i = 0; i < 8; i++)
      printf(" %u", t[i]);
   printf("\n");
}

int
main(int argc, char **argv)
{
   VkResult r;
   GET(NULL, vkCreateInstance);
   VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO, .apiVersion = VK_API_VERSION_1_3};
   VkInstanceCreateInfo ici = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &app};
   VkInstance inst;
   if ((r = vkCreateInstance(&ici, NULL, &inst))) {
      fprintf(stderr, "imgdesc: vkCreateInstance failed (%d)\n", r);
      return 1;
   }
   GET(inst, vkEnumeratePhysicalDevices);
   uint32_t n = 1;
   VkPhysicalDevice pd;
   if ((r = vkEnumeratePhysicalDevices(inst, &n, &pd)) < 0 || n == 0) {
      fprintf(stderr, "imgdesc: no device (%d)\n", r);
      return 1;
   }
   struct radv_physical_device *pdev = radv_physical_device_from_handle(pd);
   if (pdev->info.family != CHIP_RENOIR || pdev->info.gfx_level != GFX9) {
      fprintf(stderr, "imgdesc: not Renoir (run under the amdgpu drm-shim)\n");
      return 1;
   }
   info = pdev->info;
   uint64_t max_alignment;
   addrlib = ac_addrlib_create(&info, &max_alignment);
   if (!addrlib) {
      fprintf(stderr, "imgdesc: ac_addrlib_create failed\n");
      return 1;
   }
   printf("device word1 %u max_render_backends %u gb_addr_config %u\n", (ATI_VENDOR_ID << 16) | info.pci_id,
          info.max_render_backends, info.gb_addr_config);

   char line[4096];
   while (fgets(line, sizeof(line), stdin)) {
      char *p = line;
      char kind[16];
      int used;
      if (sscanf(p, "%15s%n", kind, &used) != 1)
         continue;
      p += used;
      if (!strcmp(kind, "import")) {
         unsigned w, h, nw;
         unsigned long long tiling;
         uint32_t md[64];
         if (sscanf(p, "%u %u %llu %u%n", &w, &h, &tiling, &nw, &used) != 4 || nw > 64) {
            fprintf(stderr, "imgdesc: bad import line\n");
            return 1;
         }
         p += used;
         for (unsigned i = 0; i < nw; i++) {
            if (sscanf(p, "%i%n", (int *)&md[i], &used) != 1) {
               fprintf(stderr, "imgdesc: bad import word\n");
               return 1;
            }
            p += used;
         }
         printf("begin import %u %u %llu\n", w, h, tiling);
         import_case(w, h, tiling, nw, md);
      }
      printf("end\n");
      fflush(stdout);
   }
   return 0;
}
