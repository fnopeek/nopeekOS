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
