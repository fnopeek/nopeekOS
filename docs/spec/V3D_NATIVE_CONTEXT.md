# V3D native context

How a guest's Mesa v3d driver (OpenGL ES on the Raspberry Pi 5's V3D 7.1)
reaches the host GPU through virtio-gpu DRM native context. The wire
format follows virglrenderer's `drm_context.c` and Mesa's `vdrm`; the
commands are V3D's kernel calls (`include/uapi/drm/v3d_drm.h`), sent as
ccmds. Neither upstream project has a V3D native context; this is
nopeekOS's, shaped so it could become one.

## Context and capset

| Field | Value |
|---|---|
| capset id | `VIRTIO_GPU_CAPSET_DRM` (6) |
| `context_type` | 6 (`VIRTGPU_DRM_CONTEXT_V3D`, not assigned upstream) |
| `wire_format_version` | 1 |
| `version_major.minor.patchlevel` | 1.0.0 |

The capset's union member answers every `DRM_IOCTL_V3D_GET_PARAM` that
does not change while the host runs, so the guest asks none of them:

| Offset (in the union) | Field |
|---|---|
| 0 | `hub_ident1` |
| 4 | `hub_ident2` |
| 8 | `hub_ident3` |
| 12 | `core0_ident0` |
| 16 | `core0_ident1` |
| 20 | `core0_ident2` |
| 24 | `supports`: bit 0 TFU, 1 CSD, 2 CACHE_FLUSH, 3 PERFMON, 4 MULTISYNC_EXT, 5 CPU_QUEUE, 6 SUPER_PAGES |
| 28 | `max_perf_counters` |

The host offers TFU, CSD and CACHE_FLUSH; PERFMON, CPU_QUEUE and
MULTISYNC_EXT stay off (0).

## Shared memory

The response buffer (blob id 0) starts with:

| Offset | Field |
|---|---|
| 0 | `seqno` (`vdrm_shmem`) |
| 4 | `rsp_mem_offset` = 16 |
| 8 | `async_error`: counts submissions the host refused or that failed on the GPU |
| 12 | pad (the reset counters come from `GET_PARAM`) |

Responses follow from offset 16.

## Commands

Every ccmd starts with `vdrm_ccmd_req` (`cmd`, `len`, `seqno`, `rsp_off`),
fields little-endian, 8-byte aligned. A response starts with its length.
`ret` is 0 or a negative errno.

| cmd | Name | Request after the header | Response |
|---|---|---|---|
| 1 | `NOP` | none | none |
| 2 | `CREATE_BO` | `size` u32, `flags` u32, `blob_id` u32, pad | none; sent with `RESOURCE_CREATE_BLOB` (`HOST3D`) |
| 3 | `GET_BO_OFFSET` | `res_id` u32, pad | `len`, `ret`, `offset` u32, pad |
| 4 | `WAIT_BO` | `res_id` u32, pad, `timeout_ns` u64 | `len`, `ret` |
| 5 | `SUBMIT_CL` | `bcl_start`, `bcl_end`, `rcl_start`, `rcl_end`, `qma`, `qms`, `qts`, `flags`, `nr_res` (u32 each), pad, then `nr_res` res_ids (u32) | none |
| 6 | `SUBMIT_TFU` | `icfg`, `iia`, `iis`, `ica`, `iua`, `ioa`, `ios`, `coef[4]`, `ioc` (V3D 7.1, u32 each), `res_ids[4]` (0 = none) | none |
| 7 | `SUBMIT_CSD` | `cfg[7]`, `coef[4]` (u32 each), `flags`, `nr_res`, pad, then `nr_res` res_ids | none |
| 8 | `GET_PARAM` | `param` u32, pad | `len`, `ret`, `value` u64 |

- **Addresses.** `CREATE_BO` makes an object of at least `size` bytes,
  zeroed; when it cannot, there is no object, and the guest's
  `RESOURCE_CREATE_BLOB` for that blob id fails. `GET_BO_OFFSET` gives its V3D address in the guest context's
  own address space: every context has a page table of its own, and only
  its objects (and resources attached to it) are in it. Addresses are
  stable for the object's life.
- **Memory.** The guest maps an object through `MAP_BLOB`; the host maps
  it uncached (`VIRTIO_GPU_MAP_CACHE_WC`), as V3D does not snoop the
  CPU's caches.
- **Requests.** Every other ccmd goes on ring 0; one with a response
  (`GET_BO_OFFSET`, `WAIT_BO`, `GET_PARAM`) is answered in the shared
  memory before `seqno` moves past it.
- **Submissions.** `SUBMIT_*` run in the order sent on the context's ring
  (`ring_idx` 1), each after the one before finished; the fence of the
  SUBMIT_3D that carried one, on that ring, signals when its jobs are done. `res_ids` name the
  objects the job reads or writes: unknown ones refuse the submission
  (`async_error` counts it). `SUBMIT_CL` `flags` bit 0 is
  `DRM_V3D_SUBMIT_CL_FLUSH_CACHE`; `SUBMIT_CSD` `flags` likewise.
- **Time limit.** A job runs at most 100 ms; one that does not finish is
  stopped by a GPU reset, its submission fails, `async_error` and the
  reset counters count it. The host's own jobs are not affected
  beyond waiting for the reset.
- **`GET_PARAM`** answers `GLOBAL_RESET_COUNTER` and
  `CONTEXT_RESET_COUNTER`; anything else returns `-EINVAL` (it is in the
  capset).
- **`WAIT_BO`** returns once no submission that names the object is
  running or queued, or `-ETIME` after `timeout_ns`, counted from the
  request.
- **Shared objects.** The host knows objects by `res_id` only; a dma-buf
  shared with another guest process is synchronised by the guest kernel's
  fences on the resource, as for any virtio-gpu buffer.
