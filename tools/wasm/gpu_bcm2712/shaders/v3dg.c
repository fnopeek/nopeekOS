/*
 * v3dg: build graphics pipelines (vertex + fragment shader, one per
 * fragment shader kind) for the Raspberry Pi 5's V3D 7.1 with v3dv, the way
 * a Vulkan application gets them, record one render pass that draws the
 * given rectangles, and write what v3dv built for it: the QPU programs,
 * their uniform lists, and the render job (binning and render control
 * lists, the indirect buffer with the shader records and uniform streams,
 * the tile allocation and tile state).
 *
 * It runs v3dv against Mesa's v3d drm-shim (LD_PRELOAD
 * libv3d_noop_drm_shim.so; V3D 7.1 is the shim's default). The job is
 * submitted to the shim, which does nothing with it, so that V3D_DEBUG=cl
 * prints Mesa's own decode of the control lists.
 *
 * The pipeline model the module draws with: one R32_UINT colour attachment
 * (a packed XRGB8888 word per pixel), loaded and stored; no vertex buffers
 * (the vertex shader places the rectangle from push constants and
 * gl_VertexIndex); a triangle strip of four vertices; no blending (the
 * fragment shader reads the tile buffer through VK_EXT_shader_tile_image
 * and mixes exactly).
 *
 * Usage: v3dg [-a x,y,w,h] [-r rev] <vs.spv> <fs.spv> <width> <height>
 *             <out dir> <draw>...
 * A draw is "kind:x0,y0,x1,y1": the fragment shader's specialization
 * constant 0 (one pipeline per kind) and the rectangle in pixels, the
 * first four push-constant words; the others are tagged 0xc0de0000 |
 * index. -a sets the render pass's render area (default the whole image);
 * -r reports V3D 7.1 revision `rev` instead of the shim's (7.1.10 and
 * later use the shader state record with draw index). Writes
 * <out dir>/meta ("key value" lines: the device, each pipeline's programs
 * with the prog_data fields the shader state record and the varyings
 * state are built from, the job's buffers and submit parameters),
 * <stage>-<kind>.bin for each pipeline's programs, and bo-<offset>.bin for
 * each buffer of the job.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "v3dv_bo.h"
#include "v3dv_cmd_buffer.h"
#include "v3dv_device.h"
#include "v3dv_image.h"
#include "v3dv_pipeline.h"

PUBLIC VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL vk_icdGetInstanceProcAddr(VkInstance instance, const char *pName);

static void *
read_file(const char *path, size_t *size)
{
   FILE *f = fopen(path, "rb");
   if (!f)
      return NULL;
   fseek(f, 0, SEEK_END);
   long n = ftell(f);
   fseek(f, 0, SEEK_SET);
   void *buf = malloc(n);
   if (fread(buf, 1, n, f) != (size_t)n) {
      fclose(f);
      free(buf);
      return NULL;
   }
   fclose(f);
   *size = n;
   return buf;
}

static void
die(const char *what, VkResult r)
{
   fprintf(stderr, "v3dg: %s failed (%d)\n", what, r);
   exit(1);
}

/* v3dv's maxPushConstantsSize. */
#define PUSH_BYTES 256

#define GET(inst, name) PFN_##name name = (PFN_##name)vk_icdGetInstanceProcAddr(inst, #name)

static const char *out_dir;

static FILE *
out(const char *name, const char *mode)
{
   char path[4096];
   snprintf(path, sizeof(path), "%s/%s", out_dir, name);
   FILE *f = fopen(path, mode);
   if (!f) {
      fprintf(stderr, "v3dg: cannot write %s\n", path);
      exit(1);
   }
   return f;
}

static void
dump_variant(FILE *m, struct v3dv_pipeline *pl, enum broadcom_shader_stage stage, const char *name)
{
   struct v3dv_shader_variant *v = pl->shared_data->variants[stage];
   if (!v)
      return;
   struct v3dv_bo *asm_bo = pl->shared_data->assembly_bo;
   char file[64];
   snprintf(file, sizeof(file), "%s.bin", name);
   FILE *b = out(file, "wb");
   fwrite((uint8_t *)asm_bo->map + v->assembly_offset, 1, v->qpu_insts_size, b);
   fclose(b);
   fprintf(m, "%s_va %u\n", name, asm_bo->offset + v->assembly_offset);
   fprintf(m, "%s_size %u\n", name, v->qpu_insts_size);
   fprintf(m, "%s_threads %u\n", name, v->prog_data.base->threads);
   fprintf(m, "%s_single_seg %u\n", name, v->prog_data.base->single_seg);
   fprintf(m, "%s_spill_size %u\n", name, v->prog_data.base->spill_size);
   fprintf(m, "%s_tmu_dirty_rcl %u\n", name, v->prog_data.base->tmu_dirty_rcl);
   if (stage == BROADCOM_SHADER_FRAGMENT) {
      const struct v3d_fs_prog_data *fs = v->prog_data.fs;
      fprintf(m, "%s_writes_z %u\n", name, fs->writes_z);
      fprintf(m, "%s_disable_ez %u\n", name, fs->disable_ez);
      fprintf(m, "%s_uses_center_w %u\n", name, fs->uses_center_w);
      fprintf(m, "%s_uses_implicit_point_line_varyings %u\n", name, fs->uses_implicit_point_line_varyings);
      fprintf(m, "%s_lock_scoreboard_on_first_thrsw %u\n", name, fs->lock_scoreboard_on_first_thrsw);
      fprintf(m, "%s_num_inputs %u\n", name, fs->num_inputs);
      fprintf(m, "%s_uses_pid %u\n", name, fs->uses_pid);
      for (uint32_t i = 0; i < ARRAY_SIZE(fs->flat_shade_flags); i++)
         fprintf(m, "%s_varying_flags %u %u %u %u\n", name, i, fs->flat_shade_flags[i], fs->noperspective_flags[i],
                 fs->centroid_flags[i]);
   } else {
      const struct v3d_vs_prog_data *vs = v->prog_data.vs;
      fprintf(m, "%s_uses_vid %u\n", name, vs->uses_vid);
      fprintf(m, "%s_uses_iid %u\n", name, vs->uses_iid);
      fprintf(m, "%s_uses_biid %u\n", name, vs->uses_biid);
      fprintf(m, "%s_vpm_input_size %u\n", name, vs->vpm_input_size);
      fprintf(m, "%s_vpm_output_size %u\n", name, vs->vpm_output_size);
      fprintf(m, "%s_vcm_cache_size %u\n", name, vs->vcm_cache_size);
      fprintf(m, "%s_writes_psiz %u\n", name, vs->writes_psiz);
   }
   const struct v3d_uniform_list *ul = &v->prog_data.base->uniforms;
   fprintf(m, "%s_uniforms %u\n", name, ul->count);
   for (uint32_t i = 0; i < ul->count; i++)
      fprintf(m, "%s_uniform %u %u %u\n", name, i, ul->contents[i], ul->data[i]);
}

static void
dump_bos(FILE *m, const char *what, struct list_head *list)
{
   list_for_each_entry(struct v3dv_bo, bo, list, list_link) {
      fprintf(m, "bo %s %u %u\n", what, bo->offset, bo->size);
      if (!bo->map)
         continue;
      char file[64];
      snprintf(file, sizeof(file), "bo-%u.bin", bo->offset);
      FILE *b = out(file, "wb");
      fwrite(bo->map, 1, bo->size, b);
      fclose(b);
   }
}

int
main(int argc, char **argv)
{
   int have_area = 0, rev = -1;
   uint32_t area[4] = {0};
   while (argc > 2 && argv[1][0] == '-') {
      if (!strcmp(argv[1], "-a") &&
          sscanf(argv[2], "%u,%u,%u,%u", &area[0], &area[1], &area[2], &area[3]) == 4) {
         have_area = 1;
      } else if (!strcmp(argv[1], "-r")) {
         rev = atoi(argv[2]);
      } else {
         break;
      }
      argc -= 2;
      argv += 2;
   }
   if (argc < 7) {
      fprintf(stderr, "usage: v3dg [-a x,y,w,h] [-r rev] <vs.spv> <fs.spv> <width> <height> <out dir> "
                      "<kind:x0,y0,x1,y1>...\n");
      return 2;
   }
   size_t vs_size, fs_size;
   uint32_t *vs_spv = read_file(argv[1], &vs_size);
   uint32_t *fs_spv = read_file(argv[2], &fs_size);
   if (!vs_spv || !fs_spv) {
      fprintf(stderr, "v3dg: cannot read the shaders\n");
      return 1;
   }
   uint32_t width = atoi(argv[3]);
   uint32_t height = atoi(argv[4]);
   if (!have_area) {
      area[2] = width;
      area[3] = height;
   }
   const uint32_t push_bytes = PUSH_BYTES;
   out_dir = argv[5];
   uint32_t ndraws = argc - 6;
   struct draw {
      uint32_t kind;
      uint32_t rect[4];
   } *draws = calloc(ndraws, sizeof(*draws));
   for (uint32_t i = 0; i < ndraws; i++) {
      if (sscanf(argv[6 + i], "%u:%u,%u,%u,%u", &draws[i].kind, &draws[i].rect[0], &draws[i].rect[1],
                 &draws[i].rect[2], &draws[i].rect[3]) != 5) {
         fprintf(stderr, "v3dg: bad draw %s\n", argv[6 + i]);
         return 2;
      }
   }
   VkResult r;

   GET(NULL, vkCreateInstance);
   VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO, .apiVersion = VK_API_VERSION_1_3};
   VkInstanceCreateInfo ici = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &app};
   VkInstance inst;
   if ((r = vkCreateInstance(&ici, NULL, &inst)))
      die("vkCreateInstance", r);
   GET(inst, vkEnumeratePhysicalDevices);
   GET(inst, vkGetPhysicalDeviceQueueFamilyProperties);
   GET(inst, vkGetPhysicalDeviceMemoryProperties);
   GET(inst, vkCreateDevice);
   GET(inst, vkGetDeviceQueue);
   GET(inst, vkCreateShaderModule);
   GET(inst, vkCreatePipelineLayout);
   GET(inst, vkCreateRenderPass);
   GET(inst, vkCreateGraphicsPipelines);
   GET(inst, vkCreateImage);
   GET(inst, vkGetImageMemoryRequirements);
   GET(inst, vkAllocateMemory);
   GET(inst, vkBindImageMemory);
   GET(inst, vkCreateImageView);
   GET(inst, vkCreateFramebuffer);
   GET(inst, vkCreateCommandPool);
   GET(inst, vkAllocateCommandBuffers);
   GET(inst, vkBeginCommandBuffer);
   GET(inst, vkEndCommandBuffer);
   GET(inst, vkCmdBeginRenderPass);
   GET(inst, vkCmdEndRenderPass);
   GET(inst, vkCmdBindPipeline);
   GET(inst, vkCmdPushConstants);
   GET(inst, vkCmdDraw);
   GET(inst, vkQueueSubmit);
   GET(inst, vkQueueWaitIdle);

   uint32_t n = 1;
   VkPhysicalDevice pd;
   if ((r = vkEnumeratePhysicalDevices(inst, &n, &pd)) < 0 || n == 0)
      die("vkEnumeratePhysicalDevices", r);
   struct v3dv_physical_device *pdev = v3dv_physical_device_from_handle(pd);
   if (pdev->devinfo.ver != 71) {
      fprintf(stderr, "v3dg: V3D %u, not 7.1 (run under the v3d drm-shim)\n", pdev->devinfo.ver);
      return 1;
   }
   if (rev >= 0)
      pdev->devinfo.rev = rev;

   uint32_t nq = 8;
   VkQueueFamilyProperties qf[8];
   vkGetPhysicalDeviceQueueFamilyProperties(pd, &nq, qf);
   uint32_t family = UINT32_MAX;
   for (uint32_t i = 0; i < nq; i++) {
      if (qf[i].queueFlags & VK_QUEUE_GRAPHICS_BIT) {
         family = i;
         break;
      }
   }
   if (family == UINT32_MAX) {
      fprintf(stderr, "v3dg: no graphics queue family\n");
      return 1;
   }

   float prio = 1.0f;
   VkDeviceQueueCreateInfo qci = {
      .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
      .queueFamilyIndex = family,
      .queueCount = 1,
      .pQueuePriorities = &prio,
   };
   VkPhysicalDeviceVulkan12Features f12 = {
      .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_2_FEATURES,
      .bufferDeviceAddress = VK_TRUE,
   };
   VkPhysicalDeviceFeatures2 f2 = {.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2, .pNext = &f12};
   VkDeviceCreateInfo dci = {
      .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
      .pNext = &f2,
      .queueCreateInfoCount = 1,
      .pQueueCreateInfos = &qci,
   };
   VkDevice dev;
   if ((r = vkCreateDevice(pd, &dci, NULL, &dev)))
      die("vkCreateDevice", r);
   VkQueue queue;
   vkGetDeviceQueue(dev, family, 0, &queue);

   VkShaderModuleCreateInfo vsci = {
      .sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO, .codeSize = vs_size, .pCode = vs_spv};
   VkShaderModuleCreateInfo fsci = {
      .sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO, .codeSize = fs_size, .pCode = fs_spv};
   VkShaderModule vs, fs;
   if ((r = vkCreateShaderModule(dev, &vsci, NULL, &vs)))
      die("vkCreateShaderModule (vertex)", r);
   if ((r = vkCreateShaderModule(dev, &fsci, NULL, &fs)))
      die("vkCreateShaderModule (fragment)", r);

   VkPushConstantRange range = {
      .stageFlags = VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT,
      .offset = 0,
      .size = push_bytes,
   };
   VkPipelineLayoutCreateInfo plci = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
      .pushConstantRangeCount = push_bytes ? 1 : 0,
      .pPushConstantRanges = &range,
   };
   VkPipelineLayout layout;
   if ((r = vkCreatePipelineLayout(dev, &plci, NULL, &layout)))
      die("vkCreatePipelineLayout", r);

   const VkFormat format = VK_FORMAT_R32_UINT;
   VkAttachmentDescription att = {
      .format = format,
      .samples = VK_SAMPLE_COUNT_1_BIT,
      .loadOp = VK_ATTACHMENT_LOAD_OP_LOAD,
      .storeOp = VK_ATTACHMENT_STORE_OP_STORE,
      .stencilLoadOp = VK_ATTACHMENT_LOAD_OP_DONT_CARE,
      .stencilStoreOp = VK_ATTACHMENT_STORE_OP_DONT_CARE,
      .initialLayout = VK_IMAGE_LAYOUT_GENERAL,
      .finalLayout = VK_IMAGE_LAYOUT_GENERAL,
   };
   VkAttachmentReference ref = {.attachment = 0, .layout = VK_IMAGE_LAYOUT_GENERAL};
   VkSubpassDescription sub = {
      .pipelineBindPoint = VK_PIPELINE_BIND_POINT_GRAPHICS,
      .colorAttachmentCount = 1,
      .pColorAttachments = &ref,
   };
   VkRenderPassCreateInfo rpci = {
      .sType = VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO,
      .attachmentCount = 1,
      .pAttachments = &att,
      .subpassCount = 1,
      .pSubpasses = &sub,
   };
   VkRenderPass rp;
   if ((r = vkCreateRenderPass(dev, &rpci, NULL, &rp)))
      die("vkCreateRenderPass", r);

   uint32_t kind_value = 0;
   VkSpecializationMapEntry entry = {.constantID = 0, .offset = 0, .size = 4};
   VkSpecializationInfo spec = {.mapEntryCount = 1, .pMapEntries = &entry, .dataSize = 4, .pData = &kind_value};
   VkPipelineShaderStageCreateInfo stages[2] = {
      {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, .stage = VK_SHADER_STAGE_VERTEX_BIT,
       .module = vs, .pName = "main"},
      {.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO, .stage = VK_SHADER_STAGE_FRAGMENT_BIT,
       .module = fs, .pName = "main", .pSpecializationInfo = &spec},
   };
   VkPipelineVertexInputStateCreateInfo vi = {.sType = VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO};
   VkPipelineInputAssemblyStateCreateInfo ia = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO,
      .topology = VK_PRIMITIVE_TOPOLOGY_TRIANGLE_STRIP,
   };
   VkViewport vp = {.x = 0, .y = 0, .width = (float)width, .height = (float)height, .minDepth = 0, .maxDepth = 1};
   VkRect2D sc = {.offset = {0, 0}, .extent = {width, height}};
   VkPipelineViewportStateCreateInfo vps = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO,
      .viewportCount = 1,
      .pViewports = &vp,
      .scissorCount = 1,
      .pScissors = &sc,
   };
   VkPipelineRasterizationStateCreateInfo rs = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO,
      .polygonMode = VK_POLYGON_MODE_FILL,
      .cullMode = VK_CULL_MODE_NONE,
      .frontFace = VK_FRONT_FACE_COUNTER_CLOCKWISE,
      .lineWidth = 1.0f,
   };
   VkPipelineMultisampleStateCreateInfo ms = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO,
      .rasterizationSamples = VK_SAMPLE_COUNT_1_BIT,
   };
   VkPipelineColorBlendAttachmentState cba = {.colorWriteMask = VK_COLOR_COMPONENT_R_BIT};
   VkPipelineColorBlendStateCreateInfo cb = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO,
      .attachmentCount = 1,
      .pAttachments = &cba,
   };
   VkGraphicsPipelineCreateInfo gpci = {
      .sType = VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO,
      .stageCount = 2,
      .pStages = stages,
      .pVertexInputState = &vi,
      .pInputAssemblyState = &ia,
      .pViewportState = &vps,
      .pRasterizationState = &rs,
      .pMultisampleState = &ms,
      .pColorBlendState = &cb,
      .layout = layout,
      .renderPass = rp,
      .subpass = 0,
   };
   FILE *m = out("meta", "w");
   fprintf(m, "ver %u\n", pdev->devinfo.ver);
   fprintf(m, "rev %u\n", pdev->devinfo.rev);
   fprintf(m, "qpu_count %u\n", pdev->devinfo.qpu_count);
   fprintf(m, "size %u %u\n", width, height);
   fprintf(m, "area %u %u %u %u\n", area[0], area[1], area[2], area[3]);
   VkPipeline pipelines[16] = {0};
   for (uint32_t i = 0; i < ndraws; i++) {
      uint32_t k = draws[i].kind;
      if (k >= 16) {
         fprintf(stderr, "v3dg: kind %u\n", k);
         return 2;
      }
      if (pipelines[k])
         continue;
      kind_value = k;
      if ((r = vkCreateGraphicsPipelines(dev, VK_NULL_HANDLE, 1, &gpci, NULL, &pipelines[k])))
         die("vkCreateGraphicsPipelines", r);
      struct v3dv_pipeline *pl = v3dv_pipeline_from_handle(pipelines[k]);
      char name[16];
      fprintf(m, "pipeline %u\n", k);
      fprintf(m, "pipeline_va_count %u\n", pl->va_count);
      fprintf(m, "vpm_bin %u %u %u\n", pl->vpm_cfg_bin.As, pl->vpm_cfg_bin.Ve, pl->vpm_cfg_bin.Vc);
      fprintf(m, "vpm %u %u %u\n", pl->vpm_cfg.As, pl->vpm_cfg.Ve, pl->vpm_cfg.Vc);
      snprintf(name, sizeof(name), "vs-%u", k);
      dump_variant(m, pl, BROADCOM_SHADER_VERTEX, name);
      snprintf(name, sizeof(name), "cs-%u", k);
      dump_variant(m, pl, BROADCOM_SHADER_VERTEX_BIN, name);
      snprintf(name, sizeof(name), "fs-%u", k);
      dump_variant(m, pl, BROADCOM_SHADER_FRAGMENT, name);
   }

   VkImageCreateInfo imci = {
      .sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
      .imageType = VK_IMAGE_TYPE_2D,
      .format = format,
      .extent = {width, height, 1},
      .mipLevels = 1,
      .arrayLayers = 1,
      .samples = VK_SAMPLE_COUNT_1_BIT,
      .tiling = VK_IMAGE_TILING_LINEAR,
      .usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT,
      .initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
   };
   VkImage image;
   if ((r = vkCreateImage(dev, &imci, NULL, &image)))
      die("vkCreateImage", r);
   VkMemoryRequirements mr;
   vkGetImageMemoryRequirements(dev, image, &mr);
   VkPhysicalDeviceMemoryProperties mp;
   vkGetPhysicalDeviceMemoryProperties(pd, &mp);
   uint32_t type = 0;
   while (type < mp.memoryTypeCount && !(mr.memoryTypeBits & (1u << type)))
      type++;
   VkMemoryAllocateInfo mai = {
      .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO, .allocationSize = mr.size, .memoryTypeIndex = type};
   VkDeviceMemory mem;
   if ((r = vkAllocateMemory(dev, &mai, NULL, &mem)))
      die("vkAllocateMemory", r);
   if ((r = vkBindImageMemory(dev, image, mem, 0)))
      die("vkBindImageMemory", r);
   fprintf(m, "image_va %u\n", v3dv_device_memory_from_handle(mem)->bo->offset);
   fprintf(m, "image_stride %u\n", v3dv_image_from_handle(image)->planes[0].slices[0].stride);
   VkImageViewCreateInfo ivci = {
      .sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
      .image = image,
      .viewType = VK_IMAGE_VIEW_TYPE_2D,
      .format = format,
      .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1},
   };
   VkImageView view;
   if ((r = vkCreateImageView(dev, &ivci, NULL, &view)))
      die("vkCreateImageView", r);
   VkFramebufferCreateInfo fbci = {
      .sType = VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO,
      .renderPass = rp,
      .attachmentCount = 1,
      .pAttachments = &view,
      .width = width,
      .height = height,
      .layers = 1,
   };
   VkFramebuffer fb;
   if ((r = vkCreateFramebuffer(dev, &fbci, NULL, &fb)))
      die("vkCreateFramebuffer", r);

   VkCommandPoolCreateInfo cpi = {.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO, .queueFamilyIndex = family};
   VkCommandPool pool;
   if ((r = vkCreateCommandPool(dev, &cpi, NULL, &pool)))
      die("vkCreateCommandPool", r);
   VkCommandBufferAllocateInfo cbai = {
      .sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
      .commandPool = pool,
      .level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
      .commandBufferCount = 1,
   };
   VkCommandBuffer cmdb;
   if ((r = vkAllocateCommandBuffers(dev, &cbai, &cmdb)))
      die("vkAllocateCommandBuffers", r);
   VkCommandBufferBeginInfo cbbi = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
   if ((r = vkBeginCommandBuffer(cmdb, &cbbi)))
      die("vkBeginCommandBuffer", r);
   VkRenderPassBeginInfo rpbi = {
      .sType = VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO,
      .renderPass = rp,
      .framebuffer = fb,
      .renderArea = {{area[0], area[1]}, {area[2], area[3]}},
   };
   vkCmdBeginRenderPass(cmdb, &rpbi, VK_SUBPASS_CONTENTS_INLINE);
   for (uint32_t i = 0; i < ndraws; i++) {
      vkCmdBindPipeline(cmdb, VK_PIPELINE_BIND_POINT_GRAPHICS, pipelines[draws[i].kind]);
      uint32_t push[PUSH_BYTES / 4];
      for (uint32_t j = 0; j < PUSH_BYTES / 4; j++)
         push[j] = 0xc0de0000 | j;
      memcpy(push, draws[i].rect, sizeof(draws[i].rect));
      vkCmdPushConstants(cmdb, layout, VK_SHADER_STAGE_VERTEX_BIT | VK_SHADER_STAGE_FRAGMENT_BIT, 0, push_bytes,
                         push);
      vkCmdDraw(cmdb, 4, 1, 0, 0);
   }
   vkCmdEndRenderPass(cmdb);
   if ((r = vkEndCommandBuffer(cmdb)))
      die("vkEndCommandBuffer", r);

   struct v3dv_cmd_buffer *cmd = v3dv_cmd_buffer_from_handle(cmdb);
   uint32_t jobs = 0;
   list_for_each_entry(struct v3dv_job, job, &cmd->jobs, list_link) {
      fprintf(m, "job %u type %u\n", jobs++, job->type);
      if (job->type != V3DV_JOB_TYPE_GPU_CL)
         continue;
      struct v3dv_bo *bcl_first = list_first_entry(&job->bcl.bo_list, struct v3dv_bo, list_link);
      fprintf(m, "bcl_start %u\n", bcl_first->offset);
      fprintf(m, "bcl_end %u\n", job->bcl.bo->offset + v3dv_cl_offset(&job->bcl));
      fprintf(m, "rcl_start %u\n", job->rcl.bo->offset);
      fprintf(m, "rcl_end %u\n", job->rcl.bo->offset + v3dv_cl_offset(&job->rcl));
      fprintf(m, "qma %u\n", job->tile_alloc->offset);
      fprintf(m, "qms %u\n", job->tile_alloc->size);
      fprintf(m, "qts %u\n", job->tile_state->offset);
      fprintf(m, "qts_size %u\n", job->tile_state->size);
      fprintf(m, "flush_cache %u\n", job->tmu_dirty_rcl);
      dump_bos(m, "bcl", &job->bcl.bo_list);
      dump_bos(m, "rcl", &job->rcl.bo_list);
      dump_bos(m, "indirect", &job->indirect.bo_list);
   }
   fclose(m);

   VkSubmitInfo si = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO, .commandBufferCount = 1, .pCommandBuffers = &cmdb};
   if ((r = vkQueueSubmit(queue, 1, &si, VK_NULL_HANDLE)))
      die("vkQueueSubmit", r);
   vkQueueWaitIdle(queue);
   return 0;
}
