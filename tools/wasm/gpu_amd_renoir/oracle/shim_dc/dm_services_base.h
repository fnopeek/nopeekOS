/* What DC's dm_services.h provides to the DC sources the oracles compile
 * unchanged: types, allocation, asserts, delays, and register access through
 * the oracle's register file (dc_mmio_read / dc_mmio_write, defined by the
 * oracle's main). The register-field helpers come from Linux's own
 * dm_services.h and dc_helper.c, extracted at build time. */
#pragma once
#include <linux/types.h>
#include <linux/slab.h>
#include <stdarg.h>
#include "dal_types.h"
#include "include/gpio_types.h"

#define ASSERT(x) do { } while (0)
#define ASSERT_CRITICAL(x) do { } while (0)
#define BREAK_TO_DEBUGGER() do { } while (0)
#define DC_ERR(...) do { } while (0)
#define DC_ERROR(...) do { } while (0)
#define dm_error(...) do { } while (0)
#define dm_output_to_console(...) do { } while (0)
#define msleep(ms) do { } while (0)
#define udelay(us) do { } while (0)
#define fallthrough __attribute__((fallthrough))
#define __maybe_unused __attribute__((unused))
#define container_of(ptr, type, member) ((type *)((char *)(ptr) - offsetof(type, member)))

struct dc_reg_helper_state {
	bool gather_in_progress;
};
struct dc_context;
struct dc_dmub_srv {
	struct dc_reg_helper_state reg_helper_offload;
	struct dc_context *ctx;
};
struct gpio_service;
struct dc_bios;
struct dc;
struct dc_context {
	struct dc *dc;
	void *driver_context;
	enum dce_version dce_version;
	struct dc_dmub_srv *dmub_srv;
	struct gpio_service *gpio_service;
	struct dc_bios *dc_bios;
	uint32_t dc_sink_id_count;
	uint32_t dc_stream_id_count;
	uint32_t dc_edp_id_count;
	void *logger;
};

uint32_t dc_mmio_read(uint32_t addr);
void dc_mmio_write(uint32_t addr, uint32_t value);
#define dm_read_reg(ctx, address) dc_mmio_read(address)
#define dm_write_reg(ctx, address, value) dc_mmio_write(address, value)

uint32_t generic_reg_set_ex(const struct dc_context *ctx, uint32_t addr, uint32_t reg_val, int n,
			    uint8_t shift1, uint32_t mask1, uint32_t field_value1, ...);
uint32_t generic_reg_update_ex(const struct dc_context *ctx, uint32_t addr, int n,
			       uint8_t shift1, uint32_t mask1, uint32_t field_value1, ...);
