/* What the DC sources need of the kernel beyond shim_dmub/dmub_os.h, for
 * the oracle that compiles all of display/dc unchanged (main_dchw.c).
 * Delays are no-ops: the register file answers at once. */
#pragma once
#include <limits.h>
#define container_of(ptr, type, member) ((type *)((char *)(ptr) - __builtin_offsetof(type, member)))
#define BUILD_BUG_ON(x) ((void)sizeof(char[1 - 2 * !!(x)]))
#define ilog2(n) (63 - __builtin_clzll(n))
#define noinline __attribute__((noinline))
#define noinline_for_stack __attribute__((noinline))
#ifndef min
#define min(a, b) ((a) < (b) ? (a) : (b))
#endif
#define min_t(t, a, b) ((t)(a) < (t)(b) ? (t)(a) : (t)(b))
#define max_t(t, a, b) ((t)(a) > (t)(b) ? (t)(a) : (t)(b))
#define MIN(a, b) min(a, b)
#define MAX(a, b) max(a, b)
#define clamp(v, lo, hi) min(max((v), (lo)), (hi))
#define swap(a, b) do { typeof(a) __t = (a); (a) = (b); (b) = __t; } while (0)
#define ARRAY_SIZE(a) (sizeof(a) / sizeof((a)[0]))
#define struct_size(p, member, n) (sizeof(*(p)) + (n) * sizeof((p)->member[0]))
typedef uint8_t __u8; typedef uint16_t __u16; typedef uint32_t __u32; typedef uint64_t __u64;
typedef uint16_t __be16; typedef uint32_t __be32;
#define le16_add_cpu(p, v) (*(p) += (v))

#define kmalloc(n, f) malloc(n)
#define kvzalloc(n, f) calloc(1, (n))
#define kvmalloc(n, f) malloc(n)
#define kvfree(p) free((void *)(p))
#define kmemdup(p, n, f) memcpy(malloc(n), (p), (n))
#define krealloc(p, n, f) realloc((p), (n))
#define krealloc_array(p, n, s, f) realloc((p), (n) * (s))
#define kref_init(k) ((k)->refcount = 1)
#define kref_get(k) ((k)->refcount++)
#define kref_put(k, rel) (--(k)->refcount == 0 ? (rel(k), 1) : 0)
struct mutex { int x; };

#define msleep(ms) do { } while (0)
#define fsleep(us) do { } while (0)
#define udelay(us) do { } while (0)
#define usleep_range(a, b) do { } while (0)
#define usleep_range_state(a, b, s) do { } while (0)

#define drm_dbg(...) do { } while (0)
#define drm_dbg_dp(...) do { } while (0)
#define drm_dbg_kms(...) do { } while (0)
#define drm_info(...) do { } while (0)
#define drm_warn(...) do { } while (0)
#define drm_err(...) do { } while (0)
#define pr_debug(...) do { } while (0)
#define pr_info(...) do { } while (0)
#define pr_err(...) do { } while (0)
#define DC_ERR(...) do { } while (0)
#define dm_error(...) do { } while (0)
#define dm_output_to_console(...) do { } while (0)
#define adev_to_drm(a) NULL
#define trace_dcn_optc_lock_unlock_state(...) do { } while (0)
#define trace_amdgpu_dm_dc_pipe_state(...) do { } while (0)
#define trace_amdgpu_dm_dc_clocks_state(...) do { } while (0)
#define trace_amdgpu_dm_dce_clocks_state(...) do { } while (0)
#define trace_amdgpu_dc_rreg(...) do { } while (0)
#define trace_amdgpu_dc_wreg(...) do { } while (0)
#define DC_FP_START() do { } while (0)
#define DC_FP_END() do { } while (0)
void dc_assert_fp_enabled(void);

struct drm_dsc_config;
struct drm_dsc_picture_parameter_set;
void drm_dsc_pps_payload_pack(struct drm_dsc_picture_parameter_set *pps, const struct drm_dsc_config *cfg);
int drm_dsc_compute_rc_parameters(struct drm_dsc_config *vdsc_cfg);
