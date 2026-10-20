/* The oracle for stage 2: Linux's soc15_common_hw_init with the nbio_v7_0
 * functions it calls, copied verbatim by extract.py, after the discovery
 * oracle's amdgpu_discovery_reg_base_init has set the register bases from
 * the IP discovery binary, and vega10_doorbell_index_init and
 * nbio_v7_0_set_reg_remap the fields it reads. The registers are a file in
 * which a register never written reads SEED(reg), by the rule argv[2] names
 * (ones, zeros or hash), as the port's host check reads them; every write
 * is printed. */
#define ORACLE_DISCOVERY_LIBRARY
#include "main_discovery.c"

#include "soc15_common.h"
#include "nbio/nbio_7_0_offset.h"
#include "nbio/nbio_7_0_sh_mask.h"
#include "nbio/nbio_7_0_smn.h"

static char seed_rule;
#define SEED(reg) (seed_rule == 'o' ? 0xffffffffu : seed_rule == 'z' ? 0u : \
		   (((u32)(reg) * 0x9e3779b1u) ^ 0xa5a5a5a5u))
struct regfile { u32 reg, v; };
static struct regfile direct[8192], indirect[1024];
static int ndirect, nindirect;
static u32 pcie_index, pcie_data;

static u32 file_get(struct regfile *f, int n, u32 reg)
{
	for (int i = 0; i < n; i++)
		if (f[i].reg == reg)
			return f[i].v;
	return SEED(reg);
}

static void file_set(struct regfile *f, int *n, u32 reg, u32 v)
{
	int i;

	for (i = 0; i < *n; i++)
		if (f[i].reg == reg)
			break;
	if (i == *n) {
		if (*n == 8192) {
			printf("register file full\n");
			abort();
		}
		(*n)++;
	}
	f[i].reg = reg;
	f[i].v = v;
}

/* BAR5 at address 0: the PCIe index/data pair reaches a space of its own. */
static u32 readl(const void *p)
{
	u32 reg = (u32)(uintptr_t)p / 4;

	if (reg == pcie_data)
		return file_get(indirect, nindirect, file_get(direct, ndirect, pcie_index));
	return file_get(direct, ndirect, reg);
}

static void writel(u32 v, void *p)
{
	u32 reg = (u32)(uintptr_t)p / 4;

	printf("write reg 0x%x = 0x%x\n", reg, v);
	if (reg == pcie_data)
		file_set(indirect, &nindirect, file_get(direct, ndirect, pcie_index), v);
	else
		file_set(direct, &ndirect, reg, v);
}

#define __iomem
#define spin_lock_irqsave(l, f) ((void)(f))
#define spin_unlock_irqrestore(l, f) ((void)(f))
u32 amdgpu_device_indirect_rreg(struct amdgpu_device *adev, u32 reg_addr);
void amdgpu_device_indirect_wreg(struct amdgpu_device *adev, u32 reg_addr, u32 reg_data);
/* amdgpu_device_rreg/wreg on bare metal: BAR5, or adev->pcie_rreg/wreg. */
#undef RREG32
#define RREG32(r) ((r) * 4 < adev->rmmio_size ? readl((char *)adev->rmmio + (r) * 4) : \
		   amdgpu_device_indirect_rreg(adev, (r) * 4))
#define WREG32(r, v) ((r) * 4 < adev->rmmio_size ? writel(v, (char *)adev->rmmio + (r) * 4) : \
		      amdgpu_device_indirect_wreg(adev, (r) * 4, v))
#define amdgpu_sriov_vf(adev) 0
#define amdgpu_sriov_rreg(adev, reg, flag, hwip, inst) 0u
#define amdgpu_sriov_wreg(adev, reg, value, flag, hwip, inst) ((void)0)
#define PAGE_SIZE 4096
#define BIT(n) (1u << (n))
#define pci_name(p) ""
#define pci_upstream_bridge(p) ((struct pci_dev *)0)
#define pcie_aspm_enabled(p) false
#define amdgpu_device_aspm_support_quirk(adev) false
#define amdgpu_device_get_aspm_pdev(adev) ((struct pci_dev *)0)
static int amdgpu_aspm = -1;
/* soc15_common_hw_fini and the clock gating: bare metal, no RAS. */
#define RREG32_PCIE(reg) amdgpu_device_indirect_rreg(adev, reg)
#define WREG32_PCIE(reg, v) amdgpu_device_indirect_wreg(adev, reg, v)
#define xgpu_ai_mailbox_put_irq(adev) abort()
#define amdgpu_ras_is_supported(adev, block) 0
int amdgpu_irq_put(struct amdgpu_device *adev, struct amdgpu_irq_src *src, unsigned int type);

#include "common_extracted.c"

static const struct amdgpu_nbio_funcs nbio_v7_0_funcs = {
	.init_registers = nbio_v7_0_init_registers,
	.remap_hdp_registers = nbio_v7_0_remap_hdp_registers,
	.enable_doorbell_aperture = nbio_v7_0_enable_doorbell_aperture,
	.sdma_doorbell_range = nbio_v7_0_sdma_doorbell_range,
	.get_pcie_index_offset = nbio_v7_0_get_pcie_index_offset,
	.get_pcie_data_offset = nbio_v7_0_get_pcie_data_offset,
};

#ifndef ORACLE_COMMON_LIBRARY
int amdgpu_irq_put(struct amdgpu_device *adev, struct amdgpu_irq_src *src, unsigned int type) { abort(); }

int main(int argc, char **argv)
{
	static struct { u16 device; u8 revision; } pdev = { 0x164c, 0xc1 };
	static struct amdgpu_device adev;
	static struct amdgpu_ip_block common;
	bin_path = argv[1];
	seed_rule = argv[2][0];
	adev.pdev = (void *)&pdev;
	adev.asic_type = CHIP_RENOIR;
	adev.apu_flags = AMD_APU_IS_RENOIR;
	adev.flags = CHIP_RENOIR | AMD_IS_APU;
	adev.rmmio_base = 0xfd400000;
	adev.rmmio_size = 512 << 10;
	if (amdgpu_discovery_reg_base_init(&adev))
		return 1;
	/* amdgpu_discovery_set_ip_blocks for NBIO 2.5.0, soc15_common_early_init,
	 * soc15_set_ip_blocks' init_doorbell_index */
	adev.nbio.funcs = &nbio_v7_0_funcs;
	nbio_v7_0_set_reg_remap(&adev);
	vega10_doorbell_index_init(&adev);
	pcie_index = nbio_v7_0_get_pcie_index_offset(&adev);
	pcie_data = nbio_v7_0_get_pcie_data_offset(&adev);
	common.adev = &adev;
	int r = soc15_common_hw_init(&common);
	printf("pcie_index 0x%x pcie_data 0x%x\n", pcie_index, pcie_data);
	printf("soc15_common_hw_init %d\n", r);
	return 0;
}
#endif
