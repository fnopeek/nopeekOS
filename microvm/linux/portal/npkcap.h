/*
 * The guest end of the capture port "npk.capture", shared by portal-npk and
 * npk-capture-probe. The host end and the authoritative description are in
 * kernel/src/microvm/devices/virtio_console_pci.rs and
 * kernel/src/shade/capture.rs.
 *
 * Frames: u16 len | u8 type | payload, little-endian, len counts the type
 * and is at most CAP_MAX_FRAME.
 */
#ifndef NPKCAP_H
#define NPKCAP_H

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include <gbm.h>
#include <virtgpu_drm.h>
#include <xf86drm.h>

/* Guest -> host. */
#define CAP_START 0x81
#define CAP_BUFFER 0x82
#define CAP_QUEUE 0x83
#define CAP_STOP 0x84
/* Host -> guest. */
#define CAP_STARTED 0x01
#define CAP_ENDED 0x02
#define CAP_ACCEPTED 0x03
#define CAP_REFUSED 0x04
#define CAP_FRAME 0x05
#define CAP_MAX_FRAME 32
#define FRAME_TEST 1

/* CAP_ENDED reasons. */
#define END_CANCELLED 1

/* CAP_REFUSED reasons. */
#define REFUSE_NOT_STARTED 1
#define REFUSE_SLOT 2
#define REFUSE_UNKNOWN 3
#define REFUSE_SIZE 4
#define REFUSE_PITCH 5
#define REFUSE_TOO_SMALL 6
#define REFUSE_LAYOUT 7
#define REFUSE_IN_USE 8
#define REFUSE_UNBOUND 9

static inline void put16(uint8_t *p, uint32_t v) { p[0] = v; p[1] = v >> 8; }
static inline void put32(uint8_t *p, uint32_t v) { p[0] = v; p[1] = v >> 8; p[2] = v >> 16; p[3] = v >> 24; }
static inline uint32_t get16(const uint8_t *p) { return p[0] | p[1] << 8; }
static inline uint32_t get32(const uint8_t *p) { return p[0] | p[1] << 8 | p[2] << 16 | (uint32_t)p[3] << 24; }

/* /dev/vportNpM of the port named `name`, opened non-blocking, or -1. */
static inline int open_port(const char *name)
{
	DIR *dir = opendir("/sys/class/virtio-ports");
	if (dir == NULL)
		return -1;
	struct dirent *e;
	int fd = -1;
	while (fd < 0 && (e = readdir(dir)) != NULL) {
		if (e->d_name[0] == '.')
			continue;
		char path[300], got[64] = {0};
		snprintf(path, sizeof(path), "/sys/class/virtio-ports/%s/name", e->d_name);
		FILE *f = fopen(path, "r");
		if (f == NULL)
			continue;
		if (fgets(got, sizeof(got), f) != NULL) {
			got[strcspn(got, "\n")] = 0;
			if (strcmp(got, name) == 0) {
				snprintf(path, sizeof(path), "/dev/%s", e->d_name);
				fd = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
			}
		}
		fclose(f);
	}
	closedir(dir);
	return fd;
}

/* One frame to the host; false if it did not go out whole. */
static inline int port_send(int fd, uint8_t type, const uint8_t *payload, uint16_t n)
{
	if (fd < 0 || n + 1 > CAP_MAX_FRAME)
		return 0;
	uint8_t msg[3 + CAP_MAX_FRAME];
	put16(msg, n + 1);
	msg[2] = type;
	if (n)
		memcpy(msg + 3, payload, n);
	return write(fd, msg, 3 + n) == 3 + n;
}

static inline uint32_t crc32_add(uint32_t crc, const uint8_t *p, size_t n)
{
	crc = ~crc;
	while (n--) {
		crc ^= *p++;
		for (int k = 0; k < 8; k++)
			crc = crc & 1 ? (crc >> 1) ^ 0xEDB88320u : crc >> 1;
	}
	return ~crc;
}

/* The CRC32 of a w x h XRGB8888 picture's rows, as the host computes it
 * (kernel/src/shade/capture.rs); `ok` is 0 when it cannot be mapped. */
static inline uint32_t bo_crc32(struct gbm_bo *bo, uint32_t w, uint32_t h, int *ok)
{
	uint32_t stride = 0;
	void *map = NULL;
	uint8_t *px = gbm_bo_map(bo, 0, 0, w, h, GBM_BO_TRANSFER_READ, &stride, &map);
	*ok = px != NULL;
	if (px == NULL)
		return 0;
	uint32_t crc = 0;
	for (uint32_t y = 0; y < h; y++)
		crc = crc32_add(crc, px + (size_t)y * stride, (size_t)w * 4);
	gbm_bo_unmap(bo, map);
	return crc;
}

/* The guest GPU's first render node as a GBM device, or NULL. */
static inline struct gbm_device *gbm_open(void)
{
	drmDevicePtr devs[8];
	int n = drmGetDevices2(0, devs, 8), fd = -1;
	for (int i = 0; i < n && fd < 0; i++)
		if (devs[i]->available_nodes & (1 << DRM_NODE_RENDER))
			fd = open(devs[i]->nodes[DRM_NODE_RENDER], O_RDWR | O_CLOEXEC);
	if (n > 0)
		drmFreeDevices(devs, n);
	return fd < 0 ? NULL : gbm_create_device(fd);
}

/* A linear XRGB8888 buffer and the virtio-gpu resource the host knows it
 * by; NULL when it cannot be made. */
static inline struct gbm_bo *bo_create(struct gbm_device *gbm, uint32_t w, uint32_t h, uint32_t *res)
{
	struct gbm_bo *bo = gbm_bo_create(gbm, w, h, GBM_FORMAT_XRGB8888, GBM_BO_USE_LINEAR | GBM_BO_USE_RENDERING);
	if (bo == NULL)
		bo = gbm_bo_create(gbm, w, h, GBM_FORMAT_XRGB8888, GBM_BO_USE_LINEAR);
	if (bo == NULL)
		return NULL;
	struct drm_virtgpu_resource_info ri = { .bo_handle = gbm_bo_get_handle(bo).u32 };
	if (drmIoctl(gbm_device_get_fd(gbm), DRM_IOCTL_VIRTGPU_RESOURCE_INFO, &ri) != 0) {
		gbm_bo_destroy(bo);
		return NULL;
	}
	*res = ri.res_handle;
	return bo;
}

#endif
