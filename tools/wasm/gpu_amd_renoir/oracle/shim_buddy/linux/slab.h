#pragma once
#include <stdlib.h>
#include <string.h>
#define GFP_KERNEL 0
struct kmem_cache { size_t size; };
#define KMEM_CACHE(s, f) ({ static struct kmem_cache c_ = { sizeof(struct s) }; &c_; })
static inline void *kmem_cache_zalloc(struct kmem_cache *c, int f) { return calloc(1, c->size); }
static inline void kmem_cache_free(struct kmem_cache *c, void *p) { free(p); }
static inline void kmem_cache_destroy(struct kmem_cache *c) { }
#define kmalloc_array(n, s, f) malloc((n) * (s))
#define kzalloc(s, f) calloc(1, s)
#define kfree(p) free(p)
