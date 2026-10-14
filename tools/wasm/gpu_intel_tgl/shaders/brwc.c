/*
 * brwc: compile one SPIR-V compute shader for Tiger Lake GT2 (8086:9a78,
 * Gen12.0) with Mesa's own Intel compiler (brw), the way anv compiles its
 * internal kernels (src/intel/vulkan/anv_internal_kernels.c) and its
 * compute pipelines (anv_shader_compile.c).
 *
 * The kernel model the module dispatches:
 *   - push constants only (one cross-thread block, CURBE), no per-thread data:
 *     the workgroup is one SIMD thread, so brw folds subgroup_id to 0;
 *   - memory through 64-bit addresses (GL_EXT_buffer_reference), i.e. A64
 *     stateless messages: no binding table, no surface states, no samplers;
 *   - no scratch, no shared memory, no constant data, no relocations
 *     (refused, the dispatcher does not provide them).
 *
 * Usage: brwc <in.spv> <out.bin> <out.meta>
 *        brwc device <wa-number>...
 * With INTEL_DEBUG=cs in the environment the generated assembly is printed.
 * out.meta is "key value" lines (see emit_meta), read by gen_shaders.py.
 * `device` prints what the dispatcher takes from Mesa's device tables: the
 * per-subslice thread count, the MOCS isl picks for internal buffers, and
 * whether each listed workaround applies, all as Mesa sees this device at
 * run time (PCI revision 1).
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "compiler/nir/nir.h"
#include "compiler/nir/nir_builder.h"
#include "compiler/spirv/nir_spirv.h"
#include "compiler/spirv/spirv_info.h"
#include "compiler/brw/brw_compiler.h"
#include "compiler/brw/brw_nir.h"
#include "compiler/intel_nir.h"
#include "dev/intel_debug.h"
#include "dev/intel_wa.h"
#include "isl/isl.h"
#include "dev/intel_device_info.h"
#include "util/ralloc.h"

#define TGL_GT2 0x9a78
#define TGL_GT2_REVISION 1
#define SIMD 16

static void
noop_log(void *data, unsigned *id, const char *fmt, ...)
{
   (void)data; (void)id; (void)fmt;
}

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

/* load_push_constant (32-bit offsets after nir_lower_explicit_io) becomes
 * the push data brw reads from the thread's payload registers, as
 * anv_nir_compute_push_layout does for a compute shader without a pushed
 * descriptor range. */
static bool
lower_push_constant(nir_builder *b, nir_intrinsic_instr *intrin, void *data)
{
   if (intrin->intrinsic != nir_intrinsic_load_push_constant)
      return false;
   b->cursor = nir_before_instr(&intrin->instr);
   nir_def *v = nir_load_push_data_intel(b, intrin->def.num_components,
                                         intrin->def.bit_size,
                                         intrin->src[0].ssa,
                                         .base = nir_intrinsic_base(intrin),
                                         .range = nir_intrinsic_range(intrin));
   nir_def_replace(&intrin->def, v);
   return true;
}

static void
emit_meta(FILE *f, const nir_shader *nir, const struct brw_cs_prog_data *cs,
          const struct glsl_type *push_type)
{
   fprintf(f, "local_size %u %u %u\n", cs->local_size[0], cs->local_size[1],
           cs->local_size[2]);
   for (unsigned simd = 8, i = 0; simd <= 32; simd *= 2, i++) {
      if (cs->prog_mask & (1u << i))
         fprintf(f, "simd %u %u\n", simd, cs->prog_offset[i]);
   }
   fprintf(f, "program_size %u\n", cs->base.program_size);
   fprintf(f, "grf_start %u\n", cs->base.dispatch_grf_start_reg);
   fprintf(f, "grf_used %u\n", cs->base.grf_used);
   fprintf(f, "cross_thread_regs %u\n", cs->push.cross_thread.regs);
   fprintf(f, "per_thread_regs %u\n", cs->push.per_thread.regs);
   fprintf(f, "uses_barrier %u\n", cs->uses_barrier);
   fprintf(f, "push_size %u\n", cs->base.push_sizes[0]);
   if (push_type && glsl_type_is_struct_or_ifc(push_type)) {
      for (unsigned i = 0; i < glsl_get_length(push_type); i++) {
         const struct glsl_type *m = glsl_get_struct_field(push_type, i);
         unsigned size = glsl_type_is_scalar(m) || glsl_type_is_vector(m) ?
                         glsl_get_explicit_size(m, false) : 8;
         fprintf(f, "push %s %u %u\n", glsl_get_struct_elem_name(push_type, i),
                 glsl_get_struct_field_offset(push_type, i), size);
      }
   }
   (void)nir;
}

/* The workarounds gpgpu.rs implements (gen_shaders.py WORKAROUNDS). */
static const struct {
   unsigned long id;
   enum intel_workaround_id wa;
} dispatch_workarounds[] = {
   { 1409600907, INTEL_WA_1409600907 },
   { 1607854226, INTEL_WA_1607854226 },
   { 16013000631, INTEL_WA_16013000631 },
};

static int
device_main(int argc, char **argv)
{
   struct intel_device_info devinfo;
   if (!intel_get_device_info_from_pci_id(TGL_GT2, &devinfo))
      return 1;
   devinfo.revision = TGL_GT2_REVISION;
   intel_device_info_init_was(&devinfo);

   struct isl_device isl;
   isl_device_init(&isl, &devinfo);

   printf("max_cs_threads %u\n", devinfo.max_cs_threads);
   printf("mocs_internal %u\n", isl_mocs(&isl, 0, false));
   for (int i = 2; i < argc; i++) {
      unsigned long id = strtoul(argv[i], NULL, 10);
      int wa = -1;
      for (unsigned w = 0; w < ARRAY_SIZE(dispatch_workarounds); w++) {
         if (dispatch_workarounds[w].id == id)
            wa = dispatch_workarounds[w].wa;
      }
      if (wa < 0) {
         fprintf(stderr, "brwc: workaround %lu is not in dispatch_workarounds\n", id);
         return 1;
      }
      printf("wa %lu %u\n", id, BITSET_TEST(devinfo.workarounds, wa) ? 1 : 0);
   }
   return 0;
}

int
main(int argc, char **argv)
{
   if (argc >= 2 && strcmp(argv[1], "device") == 0)
      return device_main(argc, argv);
   if (argc != 4) {
      fprintf(stderr, "usage: brwc <in.spv> <out.bin> <out.meta>\n");
      return 2;
   }
   size_t spv_size;
   uint32_t *spv = read_file(argv[1], &spv_size);
   if (!spv) {
      fprintf(stderr, "brwc: cannot read %s\n", argv[1]);
      return 1;
   }

   struct intel_device_info devinfo;
   if (!intel_get_device_info_from_pci_id(TGL_GT2, &devinfo)) {
      fprintf(stderr, "brwc: no device info for %04x\n", TGL_GT2);
      return 1;
   }

   process_intel_debug_variable();
   void *mem_ctx = ralloc_context(NULL);
   glsl_type_singleton_init_or_ref();
   struct brw_compiler *compiler = brw_compiler_create(mem_ctx, &devinfo);
   compiler->shader_debug_log = noop_log;
   compiler->shader_perf_log = noop_log;

   const nir_shader_compiler_options *nir_options =
      &compiler->nir_options[MESA_SHADER_COMPUTE];

   struct spirv_capabilities caps = {
      .Shader = true,
      .Int64 = true,
      .Int16 = true,
      .Int8 = true,
      .Float16 = true,
      .StorageBuffer8BitAccess = true,
      .StorageBuffer16BitAccess = true,
      .PhysicalStorageBufferAddresses = true,
      .GroupNonUniform = true,
   };
   struct spirv_to_nir_options spirv_options = {
      .environment = NIR_SPIRV_VULKAN,
      .capabilities = &caps,
      .ubo_addr_format = nir_address_format_32bit_index_offset,
      .ssbo_addr_format = nir_address_format_64bit_global,
      .phys_ssbo_addr_format = nir_address_format_64bit_global,
      .push_const_addr_format = nir_address_format_32bit_offset,
      .shared_addr_format = nir_address_format_32bit_offset,
      .global_addr_format = nir_address_format_64bit_global,
      .temp_addr_format = nir_address_format_32bit_offset,
      .constant_addr_format = nir_address_format_64bit_global,
   };

   nir_shader *nir = spirv_to_nir(spv, spv_size / 4, NULL, MESA_SHADER_COMPUTE,
                                  "main", &spirv_options, nir_options);
   if (!nir) {
      fprintf(stderr, "brwc: spirv_to_nir failed\n");
      return 1;
   }
   ralloc_steal(mem_ctx, nir);

   /* vk_spirv_to_nir */
   NIR_PASS(_, nir, nir_lower_variable_initializers, nir_var_function_temp);
   NIR_PASS(_, nir, nir_lower_returns);
   NIR_PASS(_, nir, nir_inline_functions);
   NIR_PASS(_, nir, nir_opt_copy_prop);
   NIR_PASS(_, nir, nir_opt_constant_folding);
   NIR_PASS(_, nir, nir_opt_deref);
   nir_remove_non_cmat_call_entrypoints(nir);
   NIR_PASS(_, nir, nir_lower_variable_initializers, ~0);
   NIR_PASS(_, nir, nir_split_var_copies);
   NIR_PASS(_, nir, nir_split_per_member_structs);

   const struct glsl_type *push_type = NULL;
   nir_foreach_variable_with_modes(var, nir, nir_var_mem_push_const)
      push_type = glsl_without_array(var->type);

   if (nir->info.workgroup_size_variable ||
       nir->info.workgroup_size[0] * nir->info.workgroup_size[1] *
       nir->info.workgroup_size[2] != SIMD) {
      fprintf(stderr, "brwc: the workgroup must be one SIMD%u thread\n", SIMD);
      return 1;
   }
   nir->info.min_subgroup_size = nir->info.max_subgroup_size = SIMD;

   struct brw_nir_compiler_opts opts = {};
   brw_preprocess_nir(compiler, nir, &opts);

   NIR_PASS(_, nir, nir_lower_memory_model);
   NIR_PASS(_, nir, nir_lower_explicit_io, nir_var_mem_push_const,
            nir_address_format_32bit_offset);
   NIR_PASS(_, nir, nir_lower_explicit_io, nir_var_mem_global,
            nir_address_format_64bit_global);
   NIR_PASS(_, nir, nir_lower_explicit_io, nir_var_mem_ssbo,
            nir_address_format_64bit_global);
   NIR_PASS(_, nir, nir_lower_vars_to_ssa);
   NIR_PASS(_, nir, nir_opt_copy_prop);
   NIR_PASS(_, nir, nir_opt_dce);

   nir_lower_compute_system_values_options csv = {
      .lower_cs_local_id_to_index = true,
   };
   NIR_PASS(_, nir, nir_lower_compute_system_values, &csv);

   union brw_any_prog_key key;
   memset(&key, 0, sizeof(key));
   struct brw_cs_prog_data prog_data;
   memset(&prog_data, 0, sizeof(prog_data));

   NIR_PASS(_, nir, brw_nir_lower_cs_intrinsics, &devinfo, &prog_data);
   NIR_PASS(_, nir, nir_shader_intrinsics_pass, lower_push_constant,
            nir_metadata_control_flow, NULL);

   unsigned push_end = 0;
   nir_foreach_function_impl(impl, nir) {
      nir_foreach_block(block, impl) {
         nir_foreach_instr(instr, block) {
            if (instr->type != nir_instr_type_intrinsic)
               continue;
            nir_intrinsic_instr *in = nir_instr_as_intrinsic(instr);
            if (in->intrinsic == nir_intrinsic_load_push_data_intel) {
               unsigned end = nir_intrinsic_base(in) + nir_intrinsic_range(in);
               if (end > push_end)
                  push_end = end;
            }
         }
      }
   }
   prog_data.base.push_sizes[0] = align(push_end, REG_SIZE);
   brw_cs_fill_push_const_info(&devinfo, &prog_data, -1);

   nir->global_mem_size = 0;
   nir->scratch_size = 0;
   nir->info.shared_size = 0;
   nir_shader_gather_info(nir, nir_shader_get_entrypoint(nir));

   struct genisa_stats stats;
   struct brw_compile_cs_params params = {
      .base = {
         .nir = nir,
         .key = &key.cs.base,
         .prog_data = &prog_data.base,
         .stats = &stats,
         .mem_ctx = mem_ctx,
         .debug_flag = DEBUG_CS,
      },
   };
   const unsigned *program = brw_compile(compiler, &params.base);
   if (!program) {
      fprintf(stderr, "brwc: brw_compile failed: %s\n",
              params.base.error_str ? params.base.error_str : "?");
      return 1;
   }

   if (prog_data.base.total_scratch || prog_data.base.total_shared ||
       prog_data.base.const_data_size || prog_data.base.num_relocs ||
       prog_data.push.per_thread.regs || prog_data.prog_spilled) {
      fprintf(stderr, "brwc: kernel needs scratch %u, shared %u, constant data %u, "
              "%u relocations, %u per-thread push registers or spills (mask %x); "
              "the dispatcher provides none of these\n",
              prog_data.base.total_scratch, prog_data.base.total_shared,
              prog_data.base.const_data_size, prog_data.base.num_relocs,
              prog_data.push.per_thread.regs, prog_data.prog_spilled);
      return 1;
   }

   FILE *bin = fopen(argv[2], "wb");
   fwrite(program, 1, prog_data.base.program_size, bin);
   fclose(bin);
   FILE *meta = fopen(argv[3], "w");
   emit_meta(meta, nir, &prog_data, push_type);
   fclose(meta);

   ralloc_free(mem_ctx);
   glsl_type_singleton_decref();
   return 0;
}
