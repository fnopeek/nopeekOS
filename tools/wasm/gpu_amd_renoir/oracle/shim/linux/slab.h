#pragma once
#include <linux/types.h>
#define GFP_KERNEL 0
#define kzalloc(n, f) calloc(1, (n))
#define kcalloc(n, s, f) calloc((n), (s))
#define kfree(p) free((void *)(p))
