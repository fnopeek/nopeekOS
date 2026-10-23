/* The kernel services the kernel half of the oracle calls, on libc. */
#include <linux/slab.h>
#include <linux/i2c.h>
#include <linux/mutex.h>
#include <linux/ratelimit.h>
#include <linux/moduleparam.h>
#include <linux/pci.h>
#include <linux/workqueue.h>
#include <drm/drm_print.h>

void *malloc(unsigned long);
void *calloc(unsigned long, unsigned long);
void *realloc(void *, unsigned long);
void free(void *);
int vfprintf(void *, const char *, va_list);
int vasprintf(char **, const char *, va_list);
extern void *stderr;

struct kmem_cache *kmalloc_caches[NR_KMALLOC_TYPES][KMALLOC_SHIFT_HIGH + 1];
const struct kernel_param_ops param_ops_int;
const struct bus_type pci_bus_type;
struct workqueue_struct *system_percpu_wq;
unsigned long __drm_debug;

static void *kz(size_t n, gfp_t flags)
{
	return flags & __GFP_ZERO ? calloc(1, n ? n : 1) : malloc(n ? n : 1);
}
void *__kmalloc_noprof(size_t size, gfp_t flags) { return kz(size, flags); }
void *__kmalloc_cache_noprof(struct kmem_cache *s, gfp_t flags, size_t size) { return kz(size, flags); }
void kfree(const void *p) { free((void *)p); }
void *kmemdup_noprof(const void *src, size_t len, gfp_t gfp)
{
	void *p = malloc(len ? len : 1);
	memcpy(p, src, len);
	return p;
}
void *krealloc_node_align_noprof(const void *p, size_t new_size, unsigned long align, gfp_t flags, int nid)
{
	return realloc((void *)p, new_size ? new_size : 1);
}
char *kasprintf(gfp_t gfp, const char *fmt, ...)
{
	va_list ap;
	char *s;
	va_start(ap, fmt);
	vasprintf(&s, fmt, ap);
	va_end(ap);
	return s;
}
void *memchr_inv(const void *start, int c, size_t bytes)
{
	const u8 *p = start;
	for (size_t i = 0; i < bytes; i++)
		if (p[i] != (u8)c)
			return (void *)(p + i);
	return NULL;
}
int _printk(const char *fmt, ...)
{
	va_list ap;
	va_start(ap, fmt);
	vfprintf(stderr, fmt, ap);
	va_end(ap);
	return 0;
}
void __drm_err(const char *format, ...)
{
	va_list ap;
	va_start(ap, format);
	vfprintf(stderr, format, ap);
	va_end(ap);
}
void __drm_dev_dbg(struct _ddebug *desc, const struct device *dev, enum drm_debug_category category,
		   const char *format, ...) { }
void drm_dev_printk(const struct device *dev, const char *level, const char *format, ...) { }
void drm_printf(struct drm_printer *p, const char *f, ...) { }
void _dev_err(const struct device *dev, const char *fmt, ...) { }
void _dev_warn(const struct device *dev, const char *fmt, ...) { }
void _dev_info(const struct device *dev, const char *fmt, ...) { }
void __warn_printk(const char *fmt, ...) { }
int ___ratelimit(struct ratelimit_state *rs, const char *func) { return 1; }
void __mutex_init(struct mutex *lock, const char *name, struct lock_class_key *key) { }
void mutex_lock(struct mutex *lock) { }
void mutex_unlock(struct mutex *lock) { }
int mutex_trylock(struct mutex *lock) { return 1; }
bool mutex_is_locked(struct mutex *lock) { return false; }
void usleep_range_state(unsigned long min, unsigned long max, unsigned int state) { }
bool is_acpi_device_node(const struct fwnode_handle *fwnode) { return false; }
unsigned int __sw_hweight32(unsigned int w)
{
	return __builtin_popcount(w);
}

/* i2c_transfer and __i2c_transfer (drivers/i2c/i2c-core-base.c): the
 * adapter's master_xfer, again on -EAGAIN up to adap->retries times. The
 * bus lock and the timeout in jiffies have nothing to guard here. */
int i2c_transfer(struct i2c_adapter *adap, struct i2c_msg *msgs, int num)
{
	int ret, try;

	if (!adap->algo->master_xfer)
		return -EOPNOTSUPP;
	if (!msgs || num < 1)
		return -EINVAL;
	for (ret = 0, try = 0; try <= adap->retries; try++) {
		ret = adap->algo->master_xfer(adap, msgs, num);
		if (ret != -EAGAIN)
			break;
	}
	return ret;
}

/* DRM's userspace interface: connector properties carry what EDID parsing
 * found to userspace and to nothing else. */
#include <drm/drm_property.h>
#include <drm/drm_connector.h>
int drm_property_replace_global_blob(struct drm_device *dev, struct drm_property_blob **replace, size_t length,
				     const void *data, struct drm_mode_object *obj_holds_id,
				     struct drm_property *prop_holds_id)
{
	return 0;
}
int drm_object_property_set_value(struct drm_mode_object *obj, struct drm_property *property, uint64_t val)
{
	return 0;
}
int drm_connector_set_tile_property(struct drm_connector *connector)
{
	return 0;
}
