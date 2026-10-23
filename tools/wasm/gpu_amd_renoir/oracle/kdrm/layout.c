/* The DC types that cross between the two halves of the detection oracle,
 * measured on both sides: compiled once against the kernel's headers, once
 * against shim_dchw, and once with LAYOUT_MAIN to compare the two. */
#define LAYOUT \
	S(dc_link) S(ddc_service) S(dc_context) S(dc_sink) S(dc_edid) S(dc_edid_caps) S(dc_panel_config) \
	S(aux_payload) S(i2c_command) S(i2c_payload) \
	O(dc_link, priv) O(dc_link, aux_mode) O(dc_link, ddc) O(dc_link, link_index) O(dc_link, ctx) \
	O(dc_link, connector_signal) O(dc_link, local_sink) O(dc_link, dpcd_caps) O(dc_link, wa_flags) \
	O(dc_link, psr_settings) O(dc_link, type) O(dc_link, panel_config) \
	O(ddc_service, ctx) O(ddc_service, link) O(ddc_service, ddc_pin) \
	O(dc_context, dc) O(dc_context, driver_context) \
	O(dc_sink, dc_edid) O(dc_sink, edid_caps) O(dc_sink, sink_signal) O(dc_sink, link) O(dc_sink, ctx) \
	O(dc_edid_caps, panel_patch) O(dc_edid_caps, display_name) O(dc_edid_caps, audio_modes) \
	O(dc_edid_caps, speaker_flags) O(dc_edid_caps, edid_hdmi) O(dc_edid_caps, qs_bit)

struct layout_entry { const char *name; unsigned long value; };

#ifdef LAYOUT_MAIN
#include <stdio.h>
extern const struct layout_entry kernel_layout[], shim_layout[];
extern const int kernel_layout_n;
int main(void)
{
	int bad = 0;

	for (int i = 0; i < kernel_layout_n; i++)
		if (kernel_layout[i].value != shim_layout[i].value) {
			fprintf(stderr, "layout: %s is %lu in the kernel, %lu in the shim\n", kernel_layout[i].name,
				kernel_layout[i].value, shim_layout[i].value);
			bad = 1;
		}
	return bad;
}
#else
#define S(t) { #t, sizeof(struct t) },
#define O(t, f) { #t "." #f, __builtin_offsetof(struct t, f) },
#ifdef __KERNEL__
#include "amdgpu.h"
#include "dc.h"
#include "dc/inc/core_types.h"
const struct layout_entry kernel_layout[] = { LAYOUT };
const int kernel_layout_n = sizeof(kernel_layout) / sizeof(kernel_layout[0]);
#else
#include "dc.h"
#include "core_types.h"
const struct layout_entry shim_layout[] = { LAYOUT };
#endif
#endif
