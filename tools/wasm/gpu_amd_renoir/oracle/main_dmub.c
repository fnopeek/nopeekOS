/* The DMUB oracle: the region layout of dm_dmub_sw_init, Linux's
 * dmub_srv.c functions on the device's DMCUB image (argv[1]) with the VBIOS
 * size of argv[2], rebased on the GPU address argv[3].
 * hostcheck/dmub_check.rs prints the same for the port.
 *
 * struct dmub_srv carries only the fields those functions touch; the
 * shared state block is the 256 bytes dmub_cmd.h fixes it to. */
#include <linux/sched.h>
#include <linux/slab.h>
struct dmub_shared_state_feature_block { uint8_t b[256]; };
#include "dmub_types.c"

struct dmub_srv {
	bool sw_init;
	uint32_t fw_version;
	struct dmub_fw_meta_info meta_info;
};
#include "dmub_funcs.c"

static enum dmub_window_memory_type window_memory_type[DMUB_WINDOW_TOTAL] = {
	DMUB_WINDOW_MEMORY_TYPE_FB, DMUB_WINDOW_MEMORY_TYPE_FB, DMUB_WINDOW_MEMORY_TYPE_FB,
	DMUB_WINDOW_MEMORY_TYPE_FB, DMUB_WINDOW_MEMORY_TYPE_FB, DMUB_WINDOW_MEMORY_TYPE_FB,
	DMUB_WINDOW_MEMORY_TYPE_FB, DMUB_WINDOW_MEMORY_TYPE_FB, DMUB_WINDOW_MEMORY_TYPE_FB,
	DMUB_WINDOW_MEMORY_TYPE_FB,
};

static uint8_t *load(const char *path, long *size)
{
	FILE *f = fopen(path, "rb");
	fseek(f, 0, SEEK_END);
	*size = ftell(f);
	fseek(f, 0, SEEK_SET);
	uint8_t *d = malloc(*size);
	fread(d, 1, *size, f);
	fclose(f);
	return d;
}

int main(int argc, char **argv)
{
	long size;
	const uint8_t *data = load(argv[1], &size);
	uint32_t vbios_size = strtoul(argv[2], NULL, 0);
	uint64_t gpu = strtoull(argv[3], NULL, 0);
	const struct dmcub_firmware_header_v1_0 *hdr = (const void *)data;
	static struct dmub_srv dmub = { .sw_init = true };
	struct dmub_srv_region_params region_params;
	struct dmub_srv_region_info region_info;
	struct dmub_srv_memory_params memory_params;
	struct dmub_srv_fb_info fb_info;

	/* dm_dmub_sw_init */
	memset(&region_params, 0, sizeof(region_params));
	region_params.inst_const_size = le32_to_cpu(hdr->inst_const_bytes) - PSP_HEADER_BYTES - PSP_FOOTER_BYTES;
	region_params.bss_data_size = le32_to_cpu(hdr->bss_data_bytes);
	region_params.vbios_size = vbios_size;
	region_params.fw_bss_data = region_params.bss_data_size ?
		data + le32_to_cpu(hdr->header.ucode_array_offset_bytes) + le32_to_cpu(hdr->inst_const_bytes) : NULL;
	region_params.fw_inst_const = data + le32_to_cpu(hdr->header.ucode_array_offset_bytes) + PSP_HEADER_BYTES;
	region_params.window_memory_type = window_memory_type;
	enum dmub_status status = dmub_srv_calc_region_info(&dmub, &region_params, &region_info);
	printf("region_info %d fb_size 0x%x gart_size 0x%x num_regions %u fw_version 0x%08x\n", status,
	       region_info.fb_size, region_info.gart_size, region_info.num_regions, dmub.fw_version);
	printf("meta magic 0x%08x fw_region_size 0x%x trace_buffer_size 0x%x shared_state_size 0x%x\n",
	       dmub.meta_info.magic_value, dmub.meta_info.fw_region_size, dmub.meta_info.trace_buffer_size,
	       dmub.meta_info.shared_state_size);
	for (int i = 0; i < DMUB_WINDOW_TOTAL; i++)
		printf("region %d 0x%x-0x%x\n", i, region_info.regions[i].base, region_info.regions[i].top);

	memset(&memory_params, 0, sizeof(memory_params));
	memory_params.cpu_fb_addr = NULL;
	memory_params.gpu_fb_addr = gpu;
	memory_params.region_info = &region_info;
	memory_params.window_memory_type = window_memory_type;
	status = dmub_srv_calc_mem_info(&dmub, &memory_params, &fb_info);
	printf("mem_info %d num_fb %u\n", status, fb_info.num_fb);
	for (int i = 0; i < DMUB_WINDOW_TOTAL; i++)
		printf("fb %d cpu 0x%lx gpu 0x%llx size 0x%x\n", i, (unsigned long)fb_info.fb[i].cpu_addr,
		       (unsigned long long)fb_info.fb[i].gpu_addr, fb_info.fb[i].size);
	return 0;
}
