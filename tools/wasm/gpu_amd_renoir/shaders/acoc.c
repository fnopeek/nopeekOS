/*
 * acoc: compile one SPIR-V compute shader for Renoir (GC 9.3.0, gfx90c) with
 * RADV's own compute pipeline path (ACO), the way a Vulkan application gets
 * it, and record the command stream RADV writes to dispatch it.
 *
 * It runs RADV against Mesa's amdgpu drm-shim (LD_PRELOAD
 * libamdgpu_noop_drm_shim.so, AMDGPU_GPU_ID=renoir), which stands in for
 * the kernel driver; nothing is submitted. From the pipeline it takes what
 * RADV computed for the dispatch: the program, its registers
 * (radv_shader_regs) and where the push constants go (the user SGPRs of
 * radv_userdata_locations). Then it records bind, push constants and one
 * dispatch into a command buffer and prints the dwords RADV wrote, so the
 * generator can hold the module's dispatch against them.
 *
 * The kernel model the module dispatches:
 *   - push constants, and with "image" one descriptor set holding one
 *     sampled image (binding 0), bound anew before every dispatch so that
 *     each recorded stream carries the set's address;
 *   - memory through 64-bit addresses (GL_EXT_buffer_reference);
 *   - no scratch, no LDS, no grid-size or workgroup-size SGPRs beyond
 *     what the recorded stream shows (refused otherwise).
 *
 * Usage: acoc <in.spv> <workgroup width> <push bytes> <out.bin> <out.meta> [image]
 * out.meta is "key value" lines, read by gen_shaders.py.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "radv_cmd_buffer.h"
#include "radv_descriptor_set.h"
#include "radv_image_view.h"
#include "radv_pipeline.h"
#include "radv_pipeline_compute.h"
#include "radv_queue.h"
#include "radv_shader.h"

PUBLIC VKAPI_ATTR PFN_vkVoidFunction VKAPI_CALL vk_icdGetInstanceProcAddr(VkInstance instance, const char *pName);

#define GRID_X 3
#define GRID_Y 5

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
   fprintf(stderr, "acoc: %s failed (%d)\n", what, r);
   exit(1);
}

#define GET(inst, name) PFN_##name name = (PFN_##name)vk_icdGetInstanceProcAddr(inst, #name)

int
main(int argc, char **argv)
{
   if (argc != 6 && !(argc == 7 && !strcmp(argv[6], "image"))) {
      fprintf(stderr, "usage: acoc <in.spv> <workgroup width> <push bytes> <out.bin> <out.meta> [image]\n");
      return 2;
   }
   bool image = argc == 7;
   size_t spv_size;
   uint32_t *spv = read_file(argv[1], &spv_size);
   if (!spv) {
      fprintf(stderr, "acoc: cannot read %s\n", argv[1]);
      return 1;
   }
   uint32_t width = atoi(argv[2]);
   uint32_t push_bytes = atoi(argv[3]);
   VkResult r;

   GET(NULL, vkCreateInstance);
   VkApplicationInfo app = {.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO, .apiVersion = VK_API_VERSION_1_3};
   VkInstanceCreateInfo ici = {.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &app};
   VkInstance inst;
   if ((r = vkCreateInstance(&ici, NULL, &inst)))
      die("vkCreateInstance", r);
   GET(inst, vkEnumeratePhysicalDevices);
   GET(inst, vkGetPhysicalDeviceProperties);
   GET(inst, vkGetPhysicalDeviceQueueFamilyProperties);
   GET(inst, vkCreateDevice);
   GET(inst, vkCreateShaderModule);
   GET(inst, vkCreatePipelineLayout);
   GET(inst, vkCreateComputePipelines);
   GET(inst, vkCreateCommandPool);
   GET(inst, vkAllocateCommandBuffers);
   GET(inst, vkBeginCommandBuffer);
   GET(inst, vkEndCommandBuffer);
   GET(inst, vkCmdBindPipeline);
   GET(inst, vkCmdPushConstants);
   GET(inst, vkCmdDispatch);
   GET(inst, vkCmdPipelineBarrier);
   GET(inst, vkGetDeviceQueue);
   GET(inst, vkQueueSubmit);
   GET(inst, vkQueueWaitIdle);
   GET(inst, vkCreateDescriptorSetLayout);
   GET(inst, vkCreateDescriptorPool);
   GET(inst, vkAllocateDescriptorSets);
   GET(inst, vkUpdateDescriptorSets);
   GET(inst, vkCmdBindDescriptorSets);
   GET(inst, vkCreateImage);
   GET(inst, vkGetImageMemoryRequirements);
   GET(inst, vkAllocateMemory);
   GET(inst, vkBindImageMemory);
   GET(inst, vkCreateImageView);

   uint32_t n = 1;
   VkPhysicalDevice pd;
   if ((r = vkEnumeratePhysicalDevices(inst, &n, &pd)) < 0 || n == 0)
      die("vkEnumeratePhysicalDevices", r);
   VkPhysicalDeviceProperties props;
   vkGetPhysicalDeviceProperties(pd, &props);
   struct radv_physical_device *pdev = radv_physical_device_from_handle(pd);
   if (pdev->info.family != CHIP_RENOIR || pdev->info.gfx_level != GFX9) {
      fprintf(stderr, "acoc: %s is not Renoir (run under the amdgpu drm-shim)\n", props.deviceName);
      return 1;
   }

   uint32_t nq = 8;
   VkQueueFamilyProperties qf[8];
   vkGetPhysicalDeviceQueueFamilyProperties(pd, &nq, qf);
   uint32_t family = UINT32_MAX;
   for (uint32_t i = 0; i < nq; i++) {
      if ((qf[i].queueFlags & VK_QUEUE_COMPUTE_BIT) && !(qf[i].queueFlags & VK_QUEUE_GRAPHICS_BIT)) {
         family = i;
         break;
      }
   }
   if (family == UINT32_MAX) {
      fprintf(stderr, "acoc: no compute-only queue family\n");
      return 1;
   }

   VkPhysicalDeviceVulkan12Features f12 = {
      .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_2_FEATURES,
      .bufferDeviceAddress = VK_TRUE,
   };
   VkPhysicalDeviceFeatures2 f2 = {
      .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2,
      .pNext = &f12,
      .features = {.shaderInt64 = VK_TRUE},
   };
   float prio = 1.0f;
   VkDeviceQueueCreateInfo qci = {
      .sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
      .queueFamilyIndex = family,
      .queueCount = 1,
      .pQueuePriorities = &prio,
   };
   VkDeviceCreateInfo dci = {
      .sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
      .pNext = &f2,
      .queueCreateInfoCount = 1,
      .pQueueCreateInfos = &qci,
   };
   VkDevice dev;
   if ((r = vkCreateDevice(pd, &dci, NULL, &dev)))
      die("vkCreateDevice", r);

   VkShaderModuleCreateInfo smci = {
      .sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO,
      .codeSize = spv_size,
      .pCode = spv,
   };
   VkShaderModule sm;
   if ((r = vkCreateShaderModule(dev, &smci, NULL, &sm)))
      die("vkCreateShaderModule", r);

   VkPushConstantRange range = {.stageFlags = VK_SHADER_STAGE_COMPUTE_BIT, .offset = 0, .size = push_bytes};
   VkDescriptorSetLayoutBinding binding = {
      .binding = 0,
      .descriptorType = VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE,
      .descriptorCount = 1,
      .stageFlags = VK_SHADER_STAGE_COMPUTE_BIT,
   };
   VkDescriptorSetLayoutCreateInfo dslci = {
      .sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO,
      .bindingCount = 1,
      .pBindings = &binding,
   };
   VkDescriptorSetLayout set_layout;
   if (image && (r = vkCreateDescriptorSetLayout(dev, &dslci, NULL, &set_layout)))
      die("vkCreateDescriptorSetLayout", r);
   VkPipelineLayoutCreateInfo plci = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
      .setLayoutCount = image ? 1 : 0,
      .pSetLayouts = &set_layout,
      .pushConstantRangeCount = push_bytes ? 1 : 0,
      .pPushConstantRanges = &range,
   };
   VkPipelineLayout layout;
   if ((r = vkCreatePipelineLayout(dev, &plci, NULL, &layout)))
      die("vkCreatePipelineLayout", r);

   VkSpecializationMapEntry entry = {.constantID = 0, .offset = 0, .size = 4};
   VkSpecializationInfo spec = {.mapEntryCount = 1, .pMapEntries = &entry, .dataSize = 4, .pData = &width};
   VkComputePipelineCreateInfo cpci = {
      .sType = VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO,
      .stage =
         {
            .sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO,
            .stage = VK_SHADER_STAGE_COMPUTE_BIT,
            .module = sm,
            .pName = "main",
            .pSpecializationInfo = &spec,
         },
      .layout = layout,
   };
   VkPipeline pipeline;
   if ((r = vkCreateComputePipelines(dev, VK_NULL_HANDLE, 1, &cpci, NULL, &pipeline)))
      die("vkCreateComputePipelines", r);

   /* A second pipeline of the same program: what a scene switches to. */
   VkPipeline pipeline2;
   if ((r = vkCreateComputePipelines(dev, VK_NULL_HANDLE, 1, &cpci, NULL, &pipeline2)))
      die("vkCreateComputePipelines", r);

   struct radv_pipeline *pl = radv_pipeline_from_handle(pipeline);
   struct radv_shader *s = pl->shaders[MESA_SHADER_COMPUTE];
   const struct ac_shader_config *c = &s->config;
   if (c->scratch_bytes_per_wave || c->lds_size) {
      fprintf(stderr, "acoc: scratch %u bytes per wave, lds %u: the dispatcher provides neither\n",
              c->scratch_bytes_per_wave, c->lds_size);
      return 1;
   }

   FILE *bin = fopen(argv[4], "wb");
   fwrite(s->code, 1, s->code_size, bin);
   fclose(bin);

   FILE *m = fopen(argv[5], "w");
   fprintf(m, "device %s\n", props.deviceName);
   fprintf(m, "address32_hi %u\n", pdev->info.address32_hi);
   fprintf(m, "code_size %u\n", s->code_size);
   fprintf(m, "exec_size %u\n", s->exec_size);
   fprintf(m, "num_sgprs %u\n", c->num_sgprs);
   fprintf(m, "num_vgprs %u\n", c->num_vgprs);
   fprintf(m, "wave_size %u\n", s->info.wave_size);
   fprintf(m, "pgm_rsrc1 %u\n", s->regs.pgm_rsrc1);
   fprintf(m, "pgm_rsrc2 %u\n", s->regs.pgm_rsrc2);
   fprintf(m, "pgm_rsrc3 %u\n", s->regs.pgm_rsrc3);
   fprintf(m, "num_thread %u %u %u\n", s->regs.cs.compute_num_thread_x, s->regs.cs.compute_num_thread_y,
           s->regs.cs.compute_num_thread_z);
   fprintf(m, "resource_limits %u\n", s->regs.cs.compute_resource_limits);
   for (int i = 0; i < AC_UD_MAX_UD; i++) {
      const struct radv_userdata_info *u = &s->info.user_sgprs_locs.shader_data[i];
      if (u->sgpr_idx >= 0)
         fprintf(m, "user_sgpr %d %d %u\n", i, u->sgpr_idx, u->num_sgprs);
   }
   for (int i = 0; i < MAX_SETS; i++) {
      const struct radv_userdata_info *u = &s->info.user_sgprs_locs.descriptor_sets[i];
      if (u->sgpr_idx >= 0)
         fprintf(m, "set_sgpr %d %d %u\n", i, u->sgpr_idx, u->num_sgprs);
   }
   fprintf(m, "push_constant_size %u\n", s->info.push_constant_size);
   fprintf(m, "inline_push %llu\n", (unsigned long long)s->info.inline_push_constant_mask);

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
   VkCommandBuffer cb;
   if ((r = vkAllocateCommandBuffers(dev, &cbai, &cb)))
      die("vkAllocateCommandBuffers", r);
   /* Two sets of the same 16x16 image, bound in turn: every dispatch
    * finds its set changed. */
   VkDescriptorSet sets[2];
   if (image) {
      VkImageCreateInfo ici2 = {
         .sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
         .imageType = VK_IMAGE_TYPE_2D,
         .format = VK_FORMAT_B8G8R8A8_UNORM,
         .extent = {16, 16, 1},
         .mipLevels = 1,
         .arrayLayers = 1,
         .samples = VK_SAMPLE_COUNT_1_BIT,
         .tiling = VK_IMAGE_TILING_OPTIMAL,
         .usage = VK_IMAGE_USAGE_SAMPLED_BIT,
      };
      VkImage img;
      if ((r = vkCreateImage(dev, &ici2, NULL, &img)))
         die("vkCreateImage", r);
      VkMemoryRequirements req;
      vkGetImageMemoryRequirements(dev, img, &req);
      VkMemoryAllocateInfo mai = {
         .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
         .allocationSize = req.size,
         .memoryTypeIndex = __builtin_ctz(req.memoryTypeBits),
      };
      VkDeviceMemory mem;
      if ((r = vkAllocateMemory(dev, &mai, NULL, &mem)))
         die("vkAllocateMemory", r);
      if ((r = vkBindImageMemory(dev, img, mem, 0)))
         die("vkBindImageMemory", r);
      VkImageViewCreateInfo vci = {
         .sType = VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO,
         .image = img,
         .viewType = VK_IMAGE_VIEW_TYPE_2D,
         .format = VK_FORMAT_B8G8R8A8_UNORM,
         .subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1},
      };
      VkImageView view;
      if ((r = vkCreateImageView(dev, &vci, NULL, &view)))
         die("vkCreateImageView", r);
      VkDescriptorPoolSize ps = {.type = VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE, .descriptorCount = 2};
      VkDescriptorPoolCreateInfo dpci = {
         .sType = VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO,
         .maxSets = 2,
         .poolSizeCount = 1,
         .pPoolSizes = &ps,
      };
      VkDescriptorPool dpool;
      if ((r = vkCreateDescriptorPool(dev, &dpci, NULL, &dpool)))
         die("vkCreateDescriptorPool", r);
      VkDescriptorSetLayout layouts[2] = {set_layout, set_layout};
      VkDescriptorSetAllocateInfo dsai = {
         .sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO,
         .descriptorPool = dpool,
         .descriptorSetCount = 2,
         .pSetLayouts = layouts,
      };
      if ((r = vkAllocateDescriptorSets(dev, &dsai, sets)))
         die("vkAllocateDescriptorSets", r);
      VkDescriptorImageInfo dii = {.imageView = view, .imageLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL};
      for (int i = 0; i < 2; i++) {
         VkWriteDescriptorSet wds = {
            .sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET,
            .dstSet = sets[i],
            .dstBinding = 0,
            .descriptorCount = 1,
            .descriptorType = VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE,
            .pImageInfo = &dii,
         };
         vkUpdateDescriptorSets(dev, 1, &wds, 0, NULL);
      }
      struct radv_descriptor_set *set = radv_descriptor_set_from_handle(sets[0]);
      fprintf(m, "set_size %u\n", set->header.size);
      fprintf(m, "set_va %llu %llu\n", (unsigned long long)set->header.va,
              (unsigned long long)radv_descriptor_set_from_handle(sets[1])->header.va);
      fprintf(m, "set_words");
      for (uint32_t i = 0; i < set->header.size / 4; i++)
         fprintf(m, " %u", set->header.mapped_ptr[i]);
      fprintf(m, "\n");
      struct radv_image_view *iview = radv_image_view_from_handle(view);
      fprintf(m, "view_desc");
      for (int i = 0; i < 8; i++)
         fprintf(m, " %u", iview->descriptor.plane_descriptors[0][i]);
      fprintf(m, "\n");
   }
   unsigned bound = 0;
#define BIND_SET()                                                                                                     \
   do {                                                                                                                \
      if (image)                                                                                                       \
         vkCmdBindDescriptorSets(cb, VK_PIPELINE_BIND_POINT_COMPUTE, layout, 0, 1, &sets[bound++ & 1], 0, NULL);       \
   } while (0)
   VkCommandBufferBeginInfo cbbi = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
   vkBeginCommandBuffer(cb, &cbbi);
   struct radv_cmd_buffer *cmd = radv_cmd_buffer_from_handle(cb);
   uint32_t begin = cmd->cs->b->cdw;
   vkCmdBindPipeline(cb, VK_PIPELINE_BIND_POINT_COMPUTE, pipeline);
   /* Each push-constant dword holds its own index, so the stream shows
    * which word lands in which SGPR. */
   uint32_t push[64];
   for (uint32_t i = 0; i < 64; i++)
      push[i] = 0xc0de0000 | i;
   BIND_SET();
   if (push_bytes)
      vkCmdPushConstants(cb, layout, VK_SHADER_STAGE_COMPUTE_BIT, 0, push_bytes, push);
   vkCmdDispatch(cb, GRID_X, GRID_Y, 1);
   uint32_t dispatched = cmd->cs->b->cdw;
   /* What a scene puts between two dispatches: the second reads what the
    * first wrote. */
   VkMemoryBarrier mb = {
      .sType = VK_STRUCTURE_TYPE_MEMORY_BARRIER,
      .srcAccessMask = VK_ACCESS_SHADER_WRITE_BIT,
      .dstAccessMask = VK_ACCESS_SHADER_READ_BIT | VK_ACCESS_SHADER_WRITE_BIT,
   };
   vkCmdPipelineBarrier(cb, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, 0, 1, &mb, 0,
                        NULL, 0, NULL);
   /* RADV emits a barrier's flush with the next dispatch. */
   BIND_SET();
   vkCmdDispatch(cb, GRID_X, GRID_Y, 1);
   uint32_t barriered = cmd->cs->b->cdw;
   /* A scene's next dispatch of the same program, with its own push
    * constants. */
   vkCmdPipelineBarrier(cb, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, 0, 1, &mb, 0,
                        NULL, 0, NULL);
   BIND_SET();
   if (push_bytes)
      vkCmdPushConstants(cb, layout, VK_SHADER_STAGE_COMPUTE_BIT, 0, push_bytes, push);
   vkCmdDispatch(cb, GRID_X, GRID_Y, 1);
   uint32_t repushed = cmd->cs->b->cdw;
   /* And of another program. */
   vkCmdPipelineBarrier(cb, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, 0, 1, &mb, 0,
                        NULL, 0, NULL);
   vkCmdBindPipeline(cb, VK_PIPELINE_BIND_POINT_COMPUTE, pipeline2);
   BIND_SET();
   if (push_bytes)
      vkCmdPushConstants(cb, layout, VK_SHADER_STAGE_COMPUTE_BIT, 0, push_bytes, push);
   vkCmdDispatch(cb, GRID_X, GRID_Y, 1);
   uint32_t switched = cmd->cs->b->cdw;
   /* At its end: the CPU and the display read the result. */
   mb.dstAccessMask = VK_ACCESS_HOST_READ_BIT | VK_ACCESS_MEMORY_READ_BIT;
   vkCmdPipelineBarrier(cb, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, VK_PIPELINE_STAGE_HOST_BIT, 0, 1, &mb, 0, NULL, 0,
                        NULL);
   uint32_t ended_barrier = cmd->cs->b->cdw;
   vkEndCommandBuffer(cb);
   uint32_t end = cmd->cs->b->cdw;
   fprintf(m, "shader_va %llu\n", (unsigned long long)s->va);
   fprintf(m, "grid %u %u 1\n", GRID_X, GRID_Y);
   const uint32_t *buf = cmd->cs->b->buf;
   struct {
      const char *name;
      uint32_t from, to;
   } parts[] = {
      {"stream_dispatch", begin, dispatched},
      {"stream_barrier_dispatch", dispatched, barriered},
      {"stream_barrier_push_dispatch", barriered, repushed},
      {"stream_barrier_bind_dispatch", repushed, switched},
      {"stream_host_barrier", switched, ended_barrier},
      {"stream_end", ended_barrier, end},
   };
   for (unsigned p = 0; p < sizeof(parts) / sizeof(parts[0]); p++) {
      fprintf(m, "%s", parts[p].name);
      for (uint32_t i = parts[p].from; i < parts[p].to; i++)
         fprintf(m, " %u", buf[i]);
      fprintf(m, "\n");
   }

   /* The queue's preambles are built on the first submission. */
   VkQueue q;
   vkGetDeviceQueue(dev, family, 0, &q);
   VkSubmitInfo si = {.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO, .commandBufferCount = 1, .pCommandBuffers = &cb};
   if ((r = vkQueueSubmit(q, 1, &si, VK_NULL_HANDLE)))
      die("vkQueueSubmit", r);
   vkQueueWaitIdle(q);
   struct radv_queue *queue = radv_queue_from_handle(q);
   struct {
      const char *name;
      struct radv_cmd_stream *cs;
   } pre[] = {
      {"preamble_full_flush", queue->state.initial_full_flush_preamble_cs},
      {"preamble", queue->state.initial_preamble_cs},
      {"preamble_continue", queue->state.continue_preamble_cs},
   };
   for (unsigned p = 0; p < sizeof(pre) / sizeof(pre[0]); p++) {
      fprintf(m, "%s", pre[p].name);
      for (uint32_t i = 0; pre[p].cs && i < pre[p].cs->b->cdw; i++)
         fprintf(m, " %u", pre[p].cs->b->buf[i]);
      fprintf(m, "\n");
   }
   if (queue->state.descriptor_bo)
      fprintf(m, "descriptor_bo_va %llu\n", (unsigned long long)radv_buffer_get_va(queue->state.descriptor_bo));
   fclose(m);
   return 0;
}
