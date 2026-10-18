#pragma once
#include <linux/types.h>
#define str_yes_no(b) ((b) ? "yes" : "no")
static inline size_t strscpy(char *d, const char *s, size_t n) { if (!n) return 0; size_t i = 0; for (; i + 1 < n && s[i]; i++) d[i] = s[i]; d[i] = 0; return i; }
