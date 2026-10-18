#pragma once
#include <linux/types.h>
static unsigned long jiffies;
#define HZ 1000
#define time_after(a, b) ((long)((b) - (a)) < 0)
#define msleep(x) ((void)(x))
#define mdelay(x) ((void)(x))
#define udelay(x) ((void)(x))
struct mutex { int x; };
#define mutex_lock(m) ((void)(m))
#define mutex_unlock(m) ((void)(m))
#define mutex_init(m) ((void)(m))
#define pr_info(...) printf(__VA_ARGS__)
#define pr_warn(...) printf("pr_warn: " __VA_ARGS__)
#define DRM_ERROR(...) printf("DRM_ERROR: " __VA_ARGS__)
#define DRM_DEBUG(...) ((void)0)
#define DRM_DEBUG_KMS(...) ((void)0)
#define OPTIMIZER_HIDE_VAR(v) __asm__("" : "+r"(v))
#define min(a, b) ((a) < (b) ? (a) : (b))
#define max(a, b) ((a) > (b) ? (a) : (b))
#define EXPORT_SYMBOL(x)
#define WARN_ON(x) (x)
#define KERN_DEBUG ""
#define KERN_CONT ""
#define printk(...) ((void)0)
#define pr_cont(...) ((void)0)
#include <errno.h>
#define do_div(n, base) ({ u32 __r = (n) % (base); (n) /= (base); __r; })
#define lower_32_bits(n) ((u32)(n))
#define upper_32_bits(n) ((u32)((u64)(n) >> 32))
#define jiffies_to_msecs(j) ((unsigned)(j))
