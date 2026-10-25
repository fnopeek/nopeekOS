/* The kernel half of the detection oracle: DRM's EDID and DP-AUX helpers
 * and the DM functions between them and DC, compiled against the kernel's
 * own headers. DC itself is the other half; only DC's own types cross. */
#include <acpi/video.h>
#include <linux/string.h>
#include <linux/acpi.h>
#include <linux/i2c.h>
#include <drm/drm_atomic.h>
#include <drm/drm_probe_helper.h>
#include <drm/amdgpu_drm.h>
#include <drm/drm_edid.h>
#include <drm/drm_fixed.h>
#include <drm/display/drm_dp_helper.h>
#include "dm_services.h"
#include "amdgpu.h"
#include "dc.h"
#include "amdgpu_dm.h"
#include "amdgpu_dm_irq.h"
#include "amdgpu_display.h"
#include "amdgpu_dm_mst_types.h"
#include "dpcd_defs.h"
#include "dc/inc/core_types.h"
#include "dm_helpers.h"
#include "ddc_service_types.h"
#include "clk_mgr.h"

/* dm_dp_aux_transfer reads its HPD quirk off the kernel's amdgpu_device;
 * build.sh binds it to this one, where it is false as amdgpu_dm_init leaves
 * it without a DMI match. The device has no ACPI companion. */
static struct amdgpu_device kdrm_adev_mem;
static struct amdgpu_device *kdrm_adev = &kdrm_adev_mem;
static struct device kdrm_device;
static struct drm_device kdrm_ddev = { .dev = &kdrm_device };

/* No ATIF: the backlight caps it would report are all zero. */
void amdgpu_acpi_get_backlight_caps(struct amdgpu_dm_backlight_caps *caps)
{
	memset(caps, 0, sizeof(*caps));
}
uint amdgpu_dc_debug_mask;

#include "kdrm_extracted.c"

/* amdgpu_dm_connector_init's part that DC's detection sees: the hardware
 * I2C adapter (create_i2c) and, on DP and eDP, the AUX channel
 * (amdgpu_dm_initialize_dp_connector, which turns the DPCD probe off). */
void kdrm_connector_create(struct dc_link *link)
{
	struct amdgpu_dm_connector *aconnector = kzalloc(sizeof(*aconnector), GFP_KERNEL);
	struct amdgpu_i2c_adapter *i2c = kzalloc(sizeof(*i2c), GFP_KERNEL);

	i2c->base.algo = &amdgpu_dm_i2c_algo;
	snprintf(i2c->base.name, sizeof(i2c->base.name), "AMDGPU DM i2c hw bus %d", link->link_index);
	i2c_set_adapdata(&i2c->base, i2c);
	i2c->ddc_service = link->ddc;
	i2c->oem = false;
	aconnector->i2c = i2c;
	aconnector->base.dev = &kdrm_ddev;
	aconnector->base.connector_type = to_drm_connector_type(link->connector_signal);
	aconnector->base.name = kasprintf(GFP_KERNEL, "connector-%d", link->link_index);
	mutex_init(&aconnector->base.edid_override_mutex);
	aconnector->dc_link = link;
	if (aconnector->base.connector_type == DRM_MODE_CONNECTOR_DisplayPort ||
	    aconnector->base.connector_type == DRM_MODE_CONNECTOR_eDP) {
		aconnector->dm_dp_aux.aux.name = kasprintf(GFP_KERNEL, "AMDGPU DM aux hw bus %d", link->link_index);
		aconnector->dm_dp_aux.aux.transfer = dm_dp_aux_transfer;
		aconnector->dm_dp_aux.aux.drm_dev = &kdrm_ddev;
		aconnector->dm_dp_aux.ddc_service = link->ddc;
		drm_dp_aux_init(&aconnector->dm_dp_aux.aux);
		drm_dp_dpcd_set_probe(&aconnector->dm_dp_aux.aux, false);
	}
	link->priv = aconnector;
}

/* The CRTC queries of vblank handling and flip throttling, on DC's stream:
 * DM's display funcs (vblank_get_counter, page_flip_get_scanoutpos) under
 * amdgpu_display_get_crtc_scanoutpos and amdgpu_get_vblank_counter_kms,
 * with CRTC 0 showing `stream` in the mode the stream's timing came from.
 * out: dm counter, dm vbl, dm position, then ret/vpos/hpos with flags 0
 * and GET_DISTANCE_TO_VBLANKSTART, then the cooked counter. */
static const struct amdgpu_display_funcs kdrm_display_funcs = {
	.vblank_get_counter = dm_vblank_get_counter,
	.page_flip_get_scanoutpos = dm_crtc_get_scanoutpos,
};

void kdrm_crtc_queries(struct dc *dc, struct dc_stream_state *stream, int clock, int htotal, int vdisplay,
		       int vtotal, int out[10])
{
	struct amdgpu_crtc *acrtc = kzalloc(sizeof(*acrtc), GFP_KERNEL);
	struct drm_display_mode *mode = &acrtc->base.hwmode;
	u32 vbl = 0, position = 0;
	int vpos, hpos;

	kdrm_adev->dm.dc = dc;
	kdrm_adev->mode_info.num_crtc = 1;
	kdrm_adev->mode_info.funcs = &kdrm_display_funcs;
	kdrm_adev->mode_info.crtcs[0] = acrtc;
	acrtc->base.dev = &kdrm_adev->ddev;
	acrtc->dm_irq_params.stream = stream;
	mode->crtc_clock = clock;
	mode->crtc_htotal = htotal;
	mode->crtc_vdisplay = vdisplay;
	mode->crtc_vtotal = vtotal;

	out[0] = dm_vblank_get_counter(kdrm_adev, 0);
	dm_crtc_get_scanoutpos(kdrm_adev, 0, &vbl, &position);
	out[1] = vbl;
	out[2] = position;
	out[3] = amdgpu_display_get_crtc_scanoutpos(&kdrm_adev->ddev, 0, 0, &vpos, &hpos, NULL, NULL, mode);
	out[4] = vpos;
	out[5] = hpos;
	out[6] = amdgpu_display_get_crtc_scanoutpos(&kdrm_adev->ddev, 0, GET_DISTANCE_TO_VBLANKSTART, &vpos, &hpos,
						    NULL, NULL, mode);
	out[7] = vpos;
	out[8] = hpos;
	out[9] = amdgpu_get_vblank_counter_kms(&acrtc->base);
}

/* DM's backlight caps and the conversions between the user's range and
 * DC's: out[0..1] the input signal range, then convert_brightness_from_user
 * of six user levels and convert_brightness_to_user of six DC levels. */
void kdrm_backlight(int out[14])
{
	struct amdgpu_display_manager *dm = &kdrm_adev->dm;
	u32 user[6] = { 0, 1, 1000, 32768, 65534, 65535 };
	u32 pwm[6] = { 0, 3083, 3084, 3085, 32768, 65535 };

	memset(&dm->backlight_caps[0], 0, sizeof(dm->backlight_caps[0]));
	amdgpu_dm_update_backlight_caps(dm, 0);
	out[0] = dm->backlight_caps[0].min_input_signal;
	out[1] = dm->backlight_caps[0].max_input_signal;
	for (int i = 0; i < 6; i++)
		out[2 + i] = convert_brightness_from_user(&dm->backlight_caps[0], user[i]);
	for (int i = 0; i < 6; i++)
		out[8 + i] = convert_brightness_to_user(&dm->backlight_caps[0], pwm[i]);
}
