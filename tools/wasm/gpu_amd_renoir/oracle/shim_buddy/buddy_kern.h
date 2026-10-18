/* What buddy.c takes from the kernel beyond tools/include, for the host.
 * range_overflows is copied into $B/buddy/range_overflows.h by build.sh. */
#pragma once
#include <errno.h>
#include <linux/kernel.h>
#include <linux/err.h>
#include <linux/log2.h>
#include <linux/sizes.h>
#include <linux/align.h>
#include <linux/bug.h>
#include "range_overflows.h"
#ifndef WARN_ON
#define WARN_ON(c) (!!(c))
#endif
#define hweight64(x) __builtin_popcountll(x)
#define pr_info(...) printf(__VA_ARGS__)
#define __init
