/* What dmub/ and the dc_types.h it pulls in need of os_types.h, for the
 * DMUB sources the oracle compiles unchanged. */
#pragma once
#include <linux/types.h>
#include <linux/slab.h>
#define READ_ONCE(x) (x)
#define ASSERT(x) do { } while (0)
#define ASSERT_CRITICAL(x) do { } while (0)
#define BREAK_TO_DEBUGGER() do { } while (0)
#define fallthrough __attribute__((fallthrough))
#define __maybe_unused __attribute__((unused))
#include <stdarg.h>
struct kref { int refcount; };
#define max(a, b) ((a) > (b) ? (a) : (b))
#include "cgs_common.h"
#define WARN_ON(x) (!!(x))
#define WARN_ON_ONCE(x) (!!(x))
static inline uint64_t div_u64_rem(uint64_t n, uint32_t d, uint32_t *r) { *r = n % d; return n / d; }
static inline uint64_t div_u64(uint64_t n, uint32_t d) { return n / d; }
static inline uint64_t div64_u64(uint64_t n, uint64_t d) { return n / d; }
static inline uint64_t div64_u64_rem(uint64_t n, uint64_t d, uint64_t *r) { *r = n % d; return n / d; }
static inline int64_t div64_s64(int64_t n, int64_t d) { return n / d; }
static inline int64_t div_s64(int64_t n, int32_t d) { return n / d; }
static inline uint64_t ktime_get_raw_ns(void) { return 0; }
