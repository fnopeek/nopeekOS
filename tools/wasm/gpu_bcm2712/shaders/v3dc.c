/*
 * v3dc: compile one SPIR-V compute shader for the Raspberry Pi 5's V3D 7.1
 * with v3dv's own compute pipeline path, the way a Vulkan application gets
 * it, and record what v3dv writes to dispatch it.
 *
 * It runs v3dv against Mesa's v3d drm-shim (LD_PRELOAD
 * libv3d_noop_drm_shim.so; V3D 7.1 is the shim's default), which stands in
 * for the kernel driver; nothing is submitted. From the pipeline it takes
 * the QPU program and the uniform list the compiler made (each word's
 * QUNIFORM_* contents and data). Then it records bind, push constants and
 * one dispatch and prints the CSD job v3dv built: the seven CFG words of
 * drm_v3d_submit_csd and the uniform stream as written, so the generator
 * can hold the module's dispatch against them.
 *
 * The kernel model the module dispatches: push constants (QUNIFORM_UNIFORM),
 * memory through 32-bit addresses (GL_EXT_buffer_reference, v3dv keeps the
 * low half), a spill buffer of 4 * qpu_count * spill_size bytes where the
 * program spills (pipeline_check_spill_size), no shared memory (refused).
 *
 * Usage: v3dc <in.spv> <workgroup width> <push bytes> <out.bin> <out.meta>
 * out.meta is "key value" lines, read by gen_shaders.py.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "v3dv_bo.h"
#include "v3dv_cmd_buffer.h"
#include "v3dv_device.h"
#include "v3dv_pipeline.h"

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
   fprintf(stderr, "v3dc: %s failed (%d)\n", what, r);
   exit(1);
}

#define GET(inst, name) PFN_##name name = (PFN_##name)vk_icdGetInstanceProcAddr(inst, #name)

int
main(int argc, char **argv)
{
   if (argc != 6) {
      fprintf(stderr, "usage: v3dc <in.spv> <workgroup width> <push bytes> <out.bin> <out.meta>\n");
      return 2;
   }
   size_t spv_size;
   uint32_t *spv = read_file(argv[1], &spv_size);
   if (!spv) {
      fprintf(stderr, "v3dc: cannot read %s\n", argv[1]);
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

   uint32_t n = 1;
   VkPhysicalDevice pd;
   if ((r = vkEnumeratePhysicalDevices(inst, &n, &pd)) < 0 || n == 0)
      die("vkEnumeratePhysicalDevices", r);
   VkPhysicalDeviceProperties props;
   vkGetPhysicalDeviceProperties(pd, &props);
   struct v3dv_physical_device *pdev = v3dv_physical_device_from_handle(pd);
   if (pdev->devinfo.ver != 71) {
      fprintf(stderr, "v3dc: %s is V3D %u, not 7.1 (run under the v3d drm-shim)\n", props.deviceName,
              pdev->devinfo.ver);
      return 1;
   }

   uint32_t nq = 8;
   VkQueueFamilyProperties qf[8];
   vkGetPhysicalDeviceQueueFamilyProperties(pd, &nq, qf);
   uint32_t family = UINT32_MAX;
   for (uint32_t i = 0; i < nq; i++) {
      if (qf[i].queueFlags & VK_QUEUE_COMPUTE_BIT) {
         family = i;
         break;
      }
   }
   if (family == UINT32_MAX) {
      fprintf(stderr, "v3dc: no compute queue family\n");
      return 1;
   }

   VkPhysicalDeviceVulkan12Features f12 = {
      .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_VULKAN_1_2_FEATURES,
      .bufferDeviceAddress = VK_TRUE,
   };
   VkPhysicalDeviceFeatures2 f2 = {.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2, .pNext = &f12};
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
   VkPipelineLayoutCreateInfo plci = {
      .sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO,
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

   struct v3dv_pipeline *pl = v3dv_pipeline_from_handle(pipeline);
   struct v3dv_shader_variant *cs = pl->shared_data->variants[BROADCOM_SHADER_COMPUTE];
   const struct v3d_compute_prog_data *cpd = cs->prog_data.cs;
   if (cpd->shared_size) {
      fprintf(stderr, "v3dc: shared %u bytes: the dispatcher provides none\n", cpd->shared_size);
      return 1;
   }

   struct v3dv_bo *asm_bo = pl->shared_data->assembly_bo;
   if (!asm_bo->map) {
      fprintf(stderr, "v3dc: cannot map the program\n");
      return 1;
   }
   FILE *bin = fopen(argv[4], "wb");
   fwrite((uint8_t *)asm_bo->map + cs->assembly_offset, 1, cs->qpu_insts_size, bin);
   fclose(bin);

   FILE *m = fopen(argv[5], "w");
   fprintf(m, "device %s\n", props.deviceName);
   fprintf(m, "ver %u\n", pdev->devinfo.ver);
   fprintf(m, "rev %u\n", pdev->devinfo.rev);
   fprintf(m, "qpu_size %u\n", cs->qpu_insts_size);
   fprintf(m, "threads %u\n", cs->prog_data.base->threads);
   fprintf(m, "single_seg %u\n", cs->prog_data.base->single_seg);
   fprintf(m, "local_size %u %u %u\n", cpd->local_size[0], cpd->local_size[1], cpd->local_size[2]);
   fprintf(m, "can_use_supergroups %u\n", cpd->can_use_supergroups);
   fprintf(m, "has_control_barrier %u\n", cs->prog_data.base->has_control_barrier);
   const struct v3d_uniform_list *ul = &cs->prog_data.base->uniforms;
   fprintf(m, "qpu_count %u\n", pdev->devinfo.qpu_count);
   fprintf(m, "spill_size %u\n", cs->prog_data.base->spill_size);
   if (pl->spill.bo)
      fprintf(m, "spill_va %u %u\n", pl->spill.bo->offset, pl->spill.bo->size);
   fprintf(m, "uniforms %u\n", ul->count);
   for (uint32_t i = 0; i < ul->count; i++)
      fprintf(m, "uniform %u %u %u\n", i, ul->contents[i], ul->data[i]);

   /* Record: bind, push constants tagged by index, one dispatch. */
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
   VkCommandBufferBeginInfo cbbi = {.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO};
   if ((r = vkBeginCommandBuffer(cb, &cbbi)))
      die("vkBeginCommandBuffer", r);
   vkCmdBindPipeline(cb, VK_PIPELINE_BIND_POINT_COMPUTE, pipeline);
   uint32_t push[64];
   for (uint32_t i = 0; i < 64; i++)
      push[i] = 0xc0de0000 | i;
   if (push_bytes)
      vkCmdPushConstants(cb, layout, VK_SHADER_STAGE_COMPUTE_BIT, 0, push_bytes, push);
   vkCmdDispatch(cb, GRID_X, GRID_Y, 1);
   if ((r = vkEndCommandBuffer(cb)))
      die("vkEndCommandBuffer", r);

   struct v3dv_cmd_buffer *cmd = v3dv_cmd_buffer_from_handle(cb);
   struct v3dv_job *job = NULL;
   list_for_each_entry(struct v3dv_job, j, &cmd->jobs, list_link) {
      if (j->type == V3DV_JOB_TYPE_GPU_CSD)
         job = j;
   }
   if (!job) {
      fprintf(stderr, "v3dc: no CSD job recorded\n");
      return 1;
   }
   fprintf(m, "grid %u %u\n", GRID_X, GRID_Y);
   for (int i = 0; i < 7; i++)
      fprintf(m, "cfg %d %u\n", i, job->csd.submit.cfg[i]);
   fprintf(m, "assembly_va %u\n", asm_bo->offset + cs->assembly_offset);

   /* The uniform stream at cfg[6], as v3dv wrote it. */
   struct v3dv_bo *ubo = NULL;
   uint32_t ua = job->csd.submit.cfg[6];
   list_for_each_entry(struct v3dv_bo, bo, &job->indirect.bo_list, list_link) {
      if (ua >= bo->offset && ua < bo->offset + bo->size)
         ubo = bo;
   }
   if (!ubo) {
      fprintf(stderr, "v3dc: the uniform stream's buffer is not the command buffer's\n");
      return 1;
   }
   if (!ubo->map) {
      fprintf(stderr, "v3dc: cannot map the uniform stream\n");
      return 1;
   }
   const uint32_t *words = (const uint32_t *)((uint8_t *)ubo->map + (ua - ubo->offset));
   for (uint32_t i = 0; i < ul->count; i++)
      fprintf(m, "stream %u %u\n", i, words[i]);
   fclose(m);
   return 0;
}
