/* The oracle for the VRAM manager: Linux's amdgpu_vram_mgr_new and _del,
 * copied verbatim by extract.py, on Linux's buddy.c and the rbtree of
 * tools/lib, compiled whole. Replays the trace the port's VRAM manager
 * prints (stdin: "vram new <pages> align <a> type <t> flags <f> place
 * <fpfn>-<lpfn> <flags> -> ..." and "vram del id <n> ...") and prints each
 * line as the port must have printed it. The TTM structures carry only the
 * fields these functions touch; ttm_resource_init/_fini count the usage
 * as ttm_resource.c does. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include "buddy.c"
#include "vram_mgr_types.h"

typedef int pid_t;
#define TASK_COMM_LEN 16
#define PAGE_SHIFT 12
#define PAGE_SIZE (1UL << PAGE_SHIFT)
#define PFN_UP(x) (((x) + PAGE_SIZE - 1) >> PAGE_SHIFT)
#define max_t(t, a, b) ((t)(a) > (t)(b) ? (t)(a) : (t)(b))
#define mutex_lock(l) ((void)0)
#define mutex_unlock(l) ((void)0)
#define spin_lock(l) ((void)0)
#define spin_unlock(l) ((void)0)
#define dev_dbg(...) ((void)0)
typedef struct { long long counter; } atomic64_t;
#define atomic64_add(v, a) ((a)->counter += (v))
#define atomic64_sub(v, a) ((a)->counter -= (v))
#define task_pid_nr(t) 0
#define get_task_comm(c, t) ((void)0)
#define current 0
#define kzalloc(s, f) calloc(1, s)
#define kfree(p) free(p)

enum ttm_caching { ttm_uncached, ttm_write_combined, ttm_cached };
struct ttm_place { u32 fpfn, lpfn, mem_type, flags; };
struct ttm_resource_manager { u64 size, usage; void *bdev; };
struct ttm_buffer_object { int type; u32 page_alignment; struct { size_t size; } base; };
struct ttm_resource {
	unsigned long start;
	size_t size;
	u32 mem_type, placement;
	struct { enum ttm_caching caching; } bus;
};
struct amdgpu_bo { u64 flags; struct ttm_buffer_object tbo; };
#define ttm_to_amdgpu_bo(tbo) container_of(tbo, struct amdgpu_bo, tbo)

struct amdgpu_vres_task { pid_t pid; char comm[TASK_COMM_LEN]; };
struct amdgpu_vram_mgr_resource {
	struct ttm_resource base;
	struct list_head blocks;
	unsigned long flags;
	struct list_head vres_node;
	struct amdgpu_vres_task task;
};
struct amdgpu_vram_reservation { u64 start, size; struct list_head allocated, blocks; };
struct amdgpu_vram_mgr {
	struct ttm_resource_manager manager;
	struct gpu_buddy mm;
	int lock;
	struct list_head reservations_pending, reserved_pages;
	atomic64_t vis_usage;
	u64 default_page_size;
	struct list_head allocated_vres_list;
};
struct amdgpu_device {
	struct { u64 mc_vram_size, visible_vram_size; struct { bool connected_to_cpu; } xgmi;
		 const struct { unsigned int (*get_dcc_alignment)(struct amdgpu_device *); } *gmc_funcs; } gmc;
	struct { struct amdgpu_vram_mgr vram_mgr; } mman;
};
#define amdgpu_gmc_get_dcc_alignment(adev) 0u
static struct ttm_resource_manager *oracle_man;

static void ttm_resource_init(struct ttm_buffer_object *bo, const struct ttm_place *place, struct ttm_resource *res)
{
	res->start = 0;
	res->size = bo->base.size;
	res->mem_type = place->mem_type;
	res->placement = place->flags;
	res->bus.caching = ttm_cached;
	oracle_man->usage += res->size;
}

static void ttm_resource_fini(struct ttm_resource_manager *man, struct ttm_resource *res)
{
	man->usage -= res->size;
}

static u64 ttm_resource_manager_usage(struct ttm_resource_manager *man) { return man->usage; }

#include "vram_mgr_extracted.c"

static struct amdgpu_device adev;
static struct amdgpu_vram_mgr_resource *by_id[4096];
static int ids[4096];

static void report(int id, struct amdgpu_vram_mgr_resource *vres)
{
	struct gpu_buddy_block *block;

	int n = 0;

	list_for_each_entry(block, &vres->blocks, link)
		n++;
	printf(" id %d start %lu placement 0x%08x blocks %d\n", id, vres->base.start, vres->base.placement, n);
	list_for_each_entry(block, &vres->blocks, link)
		printf("  block 0x%016llX+%llu\n", (unsigned long long)amdgpu_vram_mgr_block_start(block),
		       (unsigned long long)(amdgpu_vram_mgr_block_size(block) >> PAGE_SHIFT));
}

int main(int argc, char **argv)
{
	struct amdgpu_vram_mgr *mgr = &adev.mman.vram_mgr;
	char line[4096];
	int next = 0;

	gpu_buddy_module_init();
	adev.gmc.mc_vram_size = strtoull(argv[1], NULL, 0);
	adev.gmc.visible_vram_size = strtoull(argv[2], NULL, 0);
	/* amdgpu_vram_mgr_init */
	mgr->manager.size = adev.gmc.mc_vram_size;
	INIT_LIST_HEAD(&mgr->reservations_pending);
	INIT_LIST_HEAD(&mgr->reserved_pages);
	INIT_LIST_HEAD(&mgr->allocated_vres_list);
	mgr->default_page_size = PAGE_SIZE;
	int err = gpu_buddy_init(&mgr->mm, mgr->manager.size, PAGE_SIZE);
	if (err) {
		printf("gpu_buddy_init %d\n", err);
		return 1;
	}
	oracle_man = &mgr->manager;

	while (fgets(line, sizeof(line), stdin)) {
		unsigned long pages, align, type, flags, fpfn, lpfn, pflags;
		int id;

		if (sscanf(line, "vram new %lu align %lu type %lu flags %lx place %lu-%lu %lx", &pages, &align, &type,
			   &flags, &fpfn, &lpfn, &pflags) == 7) {
			static struct amdgpu_bo bos[4096];
			struct amdgpu_bo *bo = &bos[next];
			struct ttm_place place = { .fpfn = fpfn, .lpfn = lpfn, .mem_type = 2, .flags = pflags };
			struct ttm_resource *res;

			bo->flags = flags;
			bo->tbo.type = type;
			bo->tbo.page_alignment = align;
			bo->tbo.base.size = pages << PAGE_SHIFT;
			printf("vram new %lu align %lu type %lu flags 0x%08lx place %lu-%lu 0x%08lx ->", pages, align, type,
			       flags, fpfn, lpfn, pflags);
			int r = amdgpu_vram_mgr_new(&mgr->manager, &bo->tbo, &place, &res);
			if (r) {
				printf(" err %d\n", -r);
				continue;
			}
			by_id[next] = to_amdgpu_vram_mgr_resource(res);
			report(next, by_id[next]);
			next++;
		} else if (sscanf(line, "vram del id %d", &id) == 1) {
			if (id >= next || !by_id[id]) {
				printf("vram del id %d none\n", id);
				continue;
			}
			printf("vram del");
			report(id, by_id[id]);
			amdgpu_vram_mgr_del(&mgr->manager, &by_id[id]->base);
			by_id[id] = NULL;
		}
	}
	return 0;
}
