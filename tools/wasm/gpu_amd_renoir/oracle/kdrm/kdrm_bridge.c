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
