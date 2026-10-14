/*
 * packcheck: packs the commands gpgpu.rs writes with Mesa's own generated
 * packers (gen120_pack.h), one sample each, and prints the dwords.
 * gen_shaders.py packs the same samples with the field table it generated
 * and the algorithm of gpgpu.rs `pack`, and refuses to write on a mismatch.
 */
#include <stdio.h>
#include <stdint.h>
#include <stdbool.h>
#include <assert.h>
#define __gen_address_type uint64_t
#define __gen_user_data void
static inline uint64_t __gen_combine_address(void *d, void *loc, uint64_t a, uint32_t delta) { (void)d; (void)loc; return a + delta; }
#include "gen120_pack.h"

static void dump(const char *n, uint32_t *dw, int len) {
   printf("%s", n);
   for (int i = 0; i < len; i++) printf(" %08x", dw[i]);
   printf("\n");
}

int main(void) {
   uint32_t dw[32] = {0};
   struct GFX12_STATE_BASE_ADDRESS sba = { GFX12_STATE_BASE_ADDRESS_header };
   sba.GeneralStateBaseAddress = 0; sba.GeneralStateMOCS = 4; sba.GeneralStateBufferSize = 0xfffff;
   sba.GeneralStateBaseAddressModifyEnable = 1; sba.GeneralStateBufferSizeModifyEnable = 1;
   sba.StatelessDataPortAccessMOCS = 4;
   sba.SurfaceStateBaseAddress = 0x12345000ull; sba.SurfaceStateMOCS = 4; sba.SurfaceStateBaseAddressModifyEnable = 1;
   sba.IndirectObjectBaseAddress = 0; sba.IndirectObjectMOCS = 4; sba.IndirectObjectBufferSize = 0xfffff;
   sba.IndirectObjectBaseAddressModifyEnable = 1; sba.IndirectObjectBufferSizeModifyEnable = 1;
   sba.InstructionBaseAddress = 0x12346000ull; sba.InstructionMOCS = 4; sba.InstructionBufferSize = 1;
   sba.InstructionBaseAddressModifyEnable = 1; sba.InstructionBuffersizeModifyEnable = 1;
   sba.BindlessSamplerStateBaseAddress = 0; sba.BindlessSamplerStateBufferSize = 0; sba.BindlessSamplerStateMOCS = 4;
   sba.BindlessSamplerStateBaseAddressModifyEnable = 1;
   sba.DynamicStateBaseAddress = 0x12344000ull; sba.DynamicStateBufferSize = 1; sba.DynamicStateMOCS = 4;
   sba.DynamicStateBaseAddressModifyEnable = 1; sba.DynamicStateBufferSizeModifyEnable = 1;
   sba.BindlessSurfaceStateBaseAddress = 0x12345000ull; sba.BindlessSurfaceStateSize = 0; sba.BindlessSurfaceStateMOCS = 4;
   sba.BindlessSurfaceStateBaseAddressModifyEnable = 1;
   GFX12_STATE_BASE_ADDRESS_pack(NULL, dw, &sba); dump("SBA", dw, 22);

   struct GFX12_INTERFACE_DESCRIPTOR_DATA idd = {0};
   idd.KernelStartPointer = 0x40; idd.BarrierEnable = 0; idd.CrossThreadConstantDataReadLength = 1;
   idd.ThreadPreemptionDisable = 1; idd.NumberofThreadsinGPGPUThreadGroup = 1;
   GFX12_INTERFACE_DESCRIPTOR_DATA_pack(NULL, dw, &idd); dump("IDD", dw, 8);

   struct GFX12_GPGPU_WALKER w = { GFX12_GPGPU_WALKER_header };
   w.SIMDSize = 1; w.ThreadWidthCounterMaximum = 0; w.ThreadGroupIDXDimension = 4;
   w.ThreadGroupIDYDimension = 64; w.ThreadGroupIDZDimension = 1; w.RightExecutionMask = 0xffff; w.BottomExecutionMask = 0xffffffff;
   GFX12_GPGPU_WALKER_pack(NULL, dw, &w); dump("WALKER", dw, 15);

   struct GFX12_MEDIA_VFE_STATE v = { GFX12_MEDIA_VFE_STATE_header };
   v.MaximumNumberofThreads = 335; v.NumberofURBEntries = 2; v.URBEntryAllocationSize = 2; v.CURBEAllocationSize = 2;
   GFX12_MEDIA_VFE_STATE_pack(NULL, dw, &v); dump("VFE", dw, 9);

   struct GFX12_PIPE_CONTROL pc = { GFX12_PIPE_CONTROL_header };
   pc.CommandStreamerStallEnable = 1; pc.HDCPipelineFlushEnable = 1; pc.RenderTargetCacheFlushEnable = 1;
   pc.DepthCacheFlushEnable = 1; pc.DepthStallEnable = 1;
   GFX12_PIPE_CONTROL_pack(NULL, dw, &pc); dump("PC", dw, 6);

   struct GFX12_PIPELINE_SELECT ps = { GFX12_PIPELINE_SELECT_header };
   ps.MaskBits = 0x13; ps.MediaSamplerDOPClockGateEnable = 1; ps.PipelineSelection = 2;
   GFX12_PIPELINE_SELECT_pack(NULL, dw, &ps); dump("PS", dw, 1);

   struct GFX12_MEDIA_CURBE_LOAD c = { GFX12_MEDIA_CURBE_LOAD_header };
   c.CURBEDataStartAddress = 64; c.CURBETotalDataLength = 64;
   GFX12_MEDIA_CURBE_LOAD_pack(NULL, dw, &c); dump("CURBE", dw, 4);
   struct GFX12_MEDIA_INTERFACE_DESCRIPTOR_LOAD l = { GFX12_MEDIA_INTERFACE_DESCRIPTOR_LOAD_header };
   l.InterfaceDescriptorTotalLength = 32; l.InterfaceDescriptorDataStartAddress = 0;
   GFX12_MEDIA_INTERFACE_DESCRIPTOR_LOAD_pack(NULL, dw, &l); dump("IDL", dw, 4);
   struct GFX12_MEDIA_STATE_FLUSH f = { GFX12_MEDIA_STATE_FLUSH_header };
   GFX12_MEDIA_STATE_FLUSH_pack(NULL, dw, &f); dump("MSF", dw, 2);
   return 0;
}
