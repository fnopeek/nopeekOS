//! V3D native contexts (`docs/spec/V3D_NATIVE_CONTEXT.md`): the guest's
//! Mesa v3d driver sends V3D's kernel calls as ccmds, this side runs them
//! as Linux's v3d does (`v3d_bo.c`, `v3d_submit.c`, `v3d_drv.c`
//! `v3d_get_param_ioctl`). Each client has a V3D context of its own: its
//! objects are mapped in its page table only, and its jobs run with that
//! table loaded (`Composer::submit_client`). Objects are GTT pages from
//! the kernel (`npk_gpudev_tt_alloc`), which the guest maps uncached as
//! V3D does not snoop the CPU's caches.

use crate::scene::{ClientJob, Composer};
use crate::hw::PiHw;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use drm_core::linux::Line;
use gpu_bcm2712_core::hw::{Block, Hw};
use gpu_bcm2712_core::v3d::{BinJob, TfuJob};
use gpu_bcm2712_core::v3d_sched::Fence;

const EINVAL: i32 = 22;
const ENOMEM: i32 = 12;
const ENOENT: i32 = 2;
const ETIME: i32 = 62;
const PAGE: u64 = 4096;

const CCMD_NOP: u32 = 1;
const CCMD_CREATE_BO: u32 = 2;
const CCMD_GET_BO_OFFSET: u32 = 3;
const CCMD_WAIT_BO: u32 = 4;
const CCMD_SUBMIT_CL: u32 = 5;
const CCMD_SUBMIT_TFU: u32 = 6;
const CCMD_SUBMIT_CSD: u32 = 7;
const CCMD_GET_PARAM: u32 = 8;
/// `vdrm_ccmd_req`: cmd, len, seqno, rsp_off.
const REQ_HEAD: usize = 16;
/// The reply head the kernel strips: `async_error`, flags.
const REPLY_HEAD: usize = 8;
const REPLY_HANDLER_FAILED: u32 = 1;

/// `VIRTGPU_DRM_CONTEXT_V3D` and the capset's version.
const CONTEXT_V3D: u32 = 6;
const WIRE_FORMAT_VERSION: u32 = 1;
/// `supports` bits: TFU, CSD, CACHE_FLUSH.
const SUPPORTS: u32 = 0b111;
/// `DRM_V3D_SUBMIT_CL_FLUSH_CACHE`, also for a dispatch.
const SUBMIT_FLUSH_CACHE: u32 = 1;
const PARAM_GLOBAL_RESET_COUNTER: u32 = 15;
const PARAM_CONTEXT_RESET_COUNTER: u32 = 16;
/// `VIRTIO_GPU_MAP_CACHE_WC`.
const MAP_CACHE_WC: u32 = 3;
/// The ring the guest submits on.
const RING: i32 = 1;
/// Largest object, as one `npk_v3d_ctx_map` maps at most (256 MiB).
const MAX_OBJECT_PAGES: u64 = 1 << 16;
/// Objects a submission may name.
const MAX_SUBMIT_RES: u32 = 4096;

/// One object: GTT pages mapped in the context that holds it.
struct Obj {
    /// The kernel's GTT pages; None for one attached from another client,
    /// whose owner frees them.
    tt: Option<i32>,
    va: u32,
    /// The last submission that names it.
    last: Fence,
}

struct Client {
    /// Its V3D context (`npk_v3d_ctx_create`).
    ctx: i32,
    /// Objects made by `CREATE_BO`, until they become resources.
    blobs: BTreeMap<u32, Obj>,
    resources: BTreeMap<u32, Obj>,
    /// Submissions on `RING` and how many are done, with the fence of each
    /// not yet done.
    submitted: u64,
    completed: u64,
    inflight: VecDeque<(u64, Fence)>,
    async_error: u32,
    /// Its jobs a reset found guilty (`v3d_file_priv.reset_counter`).
    resets: u64,
}

/// A `WAIT_BO` that waits: job `seq`, until `fence` signals or `until_us`.
struct Wait {
    seq: i32,
    client: u32,
    fence: Fence,
    until_us: u64,
}

#[derive(Default)]
pub struct Native {
    clients: BTreeMap<u32, Client>,
    waits: Vec<Wait>,
}

/// What a ccmd job leads to.
pub enum Done {
    /// Its reply, the job complete.
    Reply(Vec<u8>),
    /// It completes later (`poll`).
    Later,
}

/// `struct virgl_renderer_capset_drm` for V3D: the header, then what
/// `DRM_IOCTL_V3D_GET_PARAM` answers that does not change.
pub fn capset(hw: &mut PiHw) -> Vec<u8> {
    let mut c = Vec::new();
    let mut put = |v: u32| c.extend_from_slice(&v.to_le_bytes());
    for v in [WIRE_FORMAT_VERSION, 1, 0, 0, CONTEXT_V3D, 0] {
        put(v);
    }
    for (b, off) in [(Block::V3dHub, 0xc), (Block::V3dHub, 0x10), (Block::V3dHub, 0x14),
                     (Block::V3dCore, 0x0), (Block::V3dCore, 0x4), (Block::V3dCore, 0x8)] {
        put(hw.read(b, off));
    }
    put(SUPPORTS);
    put(0);
    c
}

fn d32(b: &[u8], at: usize) -> u32 {
    let mut w = [0u8; 4];
    for (i, x) in w.iter_mut().enumerate() {
        *x = b.get(at + i).copied().unwrap_or(0);
    }
    u32::from_le_bytes(w)
}

fn reply(async_error: u32, failed: bool, rsp: &[u8]) -> Vec<u8> {
    let mut r = Vec::with_capacity(REPLY_HEAD + rsp.len());
    r.extend_from_slice(&async_error.to_le_bytes());
    r.extend_from_slice(&(if failed { REPLY_HANDLER_FAILED } else { 0 }).to_le_bytes());
    r.extend_from_slice(rsp);
    r
}

/// A response: its length, `ret`, then `words`.
fn rsp(ret: i32, words: &[u32]) -> Vec<u8> {
    let len = 8 + words.len() * 4;
    let mut r = Vec::with_capacity(len);
    r.extend_from_slice(&(len as u32).to_le_bytes());
    r.extend_from_slice(&ret.to_le_bytes());
    for w in words {
        r.extend_from_slice(&w.to_le_bytes());
    }
    r
}

/// GTT pages for `pages` pages of client `client`, mapped in its context.
fn object(client: u32, ctx: i32, pages: u64) -> Result<Obj, i32> {
    if pages == 0 || pages > MAX_OBJECT_PAGES {
        return Err(-EINVAL);
    }
    let tt = npk_sys::gpudev_tt_alloc(client as i32, pages as i32);
    if tt <= 0 {
        return Err(-ENOMEM);
    }
    let list = match tt_pages(tt, pages) {
        Some(l) => l,
        None => {
            npk_sys::gpudev_tt_free(tt);
            return Err(-ENOMEM);
        }
    };
    let va = npk_sys::v3d_ctx_map(ctx, &list);
    if va <= 0 {
        npk_sys::gpudev_tt_free(tt);
        return Err(-ENOMEM);
    }
    Ok(Obj { tt: Some(tt), va: va as u32, last: 0 })
}

/// The physical pages of GTT pages `tt`, in order.
fn tt_pages(tt: i32, pages: u64) -> Option<Vec<u64>> {
    let buf = npk_sys::gpudev_tt_buffer(tt);
    if buf <= 0 {
        return None;
    }
    let mut list = alloc::vec![0u64; pages as usize];
    let mut at = 0;
    while at < list.len() {
        let n = npk_sys::gpudev_bo_pages(buf, at as i32, &mut list[at..]);
        if n <= 0 {
            return None;
        }
        at += n as usize;
    }
    Some(list)
}

/// The object gone from context `ctx` once no job reaches it.
fn free_object(c: &mut Composer, hw: &mut PiHw, ctx: i32, o: Obj) {
    if o.last != 0 && !c.fence(o.last).0 {
        c.drain(hw);
    }
    let _ = npk_sys::v3d_ctx_unmap(ctx, o.va as i64);
    if let Some(tt) = o.tt {
        npk_sys::gpudev_tt_free(tt);
    }
}

impl Native {
    /// `OP_CLIENT` open: a client with a context of its own.
    pub fn open(&mut self, id: u32) -> i32 {
        if self.clients.contains_key(&id) {
            return -EINVAL;
        }
        let ctx = npk_sys::v3d_ctx_create();
        if ctx <= 0 {
            return -ENOMEM;
        }
        self.clients.insert(id, Client {
            ctx, blobs: BTreeMap::new(), resources: BTreeMap::new(), submitted: 0, completed: 0,
            inflight: VecDeque::new(), async_error: 0, resets: 0,
        });
        0
    }

    /// `OP_CLIENT` close: its work finished, its objects and context gone.
    pub fn close(&mut self, c: &mut Composer, hw: &mut PiHw, id: u32) -> i32 {
        let Some(cl) = self.clients.remove(&id) else { return -EINVAL };
        c.drain(hw);
        let _ = c.use_ctx(hw, 0);
        for (_, o) in cl.blobs.into_iter().chain(cl.resources) {
            free_object(c, hw, cl.ctx, o);
        }
        let _ = npk_sys::v3d_ctx_destroy(cl.ctx);
        self.waits.retain(|w| w.client != id);
        0
    }

    /// `OP_CCMD`: request `req` of client `id`.
    pub fn ccmd(&mut self, c: &mut Composer, hw: &mut PiHw, seq: i32, id: u32, req: &[u8]) -> Done {
        let Some(cl) = self.clients.get_mut(&id) else { return Done::Reply(reply(0, true, &[])) };
        if req.len() < REQ_HEAD {
            return Done::Reply(reply(cl.async_error, true, &[]));
        }
        let cmd = d32(req, 0);
        let a = |i: usize| d32(req, REQ_HEAD + 4 * i);
        let out = match cmd {
            CCMD_NOP => Vec::new(),
            CCMD_CREATE_BO => {
                let (size, blob_id) = (a(0) as u64, a(2));
                if cl.blobs.contains_key(&blob_id) {
                    return Done::Reply(reply(cl.async_error, true, &[]));
                }
                // A failure leaves no object: the guest's resource for
                // `blob_id` then fails (`OP_BLOB`).
                if let Ok(o) = object(id, cl.ctx, size.div_ceil(PAGE)) {
                    cl.blobs.insert(blob_id, o);
                }
                Vec::new()
            }
            CCMD_GET_BO_OFFSET => match cl.resources.get(&a(0)) {
                Some(o) => rsp(0, &[o.va, 0]),
                None => rsp(-ENOENT, &[0, 0]),
            },
            CCMD_WAIT_BO => {
                let Some(o) = cl.resources.get(&a(0)) else {
                    return Done::Reply(reply(cl.async_error, false, &rsp(-ENOENT, &[])));
                };
                if o.last == 0 || c.fence(o.last).0 {
                    rsp(0, &[])
                } else {
                    let ns = a(2) as u64 | (a(3) as u64) << 32;
                    let until_us = (npk_sys::now_us() as u64).saturating_add(ns / 1000);
                    self.waits.push(Wait { seq, client: id, fence: o.last, until_us });
                    return Done::Later;
                }
            }
            CCMD_SUBMIT_CL | CCMD_SUBMIT_TFU | CCMD_SUBMIT_CSD => {
                submit(c, hw, cl, id, cmd, req);
                Vec::new()
            }
            CCMD_GET_PARAM => match a(0) {
                PARAM_GLOBAL_RESET_COUNTER => rsp(0, &[c.resets() as u32, (c.resets() >> 32) as u32]),
                PARAM_CONTEXT_RESET_COUNTER => rsp(0, &[cl.resets as u32, (cl.resets >> 32) as u32]),
                _ => rsp(-EINVAL, &[0, 0]),
            },
            _ => return Done::Reply(reply(cl.async_error, true, &[])),
        };
        Done::Reply(reply(cl.async_error, false, &out))
    }

    /// `OP_BLOB`: object `blob_id` of client `id` becomes resource
    /// `res_id`; its GTT pages, mapped write-combined by the guest.
    pub fn blob(&mut self, id: u32, blob_id: u32, res_id: u32) -> Result<Vec<u8>, i32> {
        let cl = self.clients.get_mut(&id).ok_or(-EINVAL)?;
        if cl.resources.contains_key(&res_id) {
            return Err(-EINVAL);
        }
        let o = cl.blobs.remove(&blob_id).ok_or(-ENOENT)?;
        let Some(tt) = o.tt else {
            cl.blobs.insert(blob_id, o);
            return Err(-EINVAL);
        };
        let mut out = Vec::new();
        for v in [MAP_CACHE_WC, 0, tt as u32, 0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        cl.resources.insert(res_id, o);
        Ok(out)
    }

    /// `OP_BLOB_RELEASE` and a detach: the guest let go of `res_id`.
    pub fn release(&mut self, c: &mut Composer, hw: &mut PiHw, id: u32, res_id: u32) -> i32 {
        let Some(cl) = self.clients.get_mut(&id) else { return -EINVAL };
        let Some(o) = cl.resources.remove(&res_id) else { return -EINVAL };
        let ctx = cl.ctx;
        free_object(c, hw, ctx, o);
        0
    }

    /// `OP_ATTACH`: client `id` uses resource `owner_res` of client
    /// `owner` under `res_id`: the same pages, mapped in its own context.
    pub fn attach(&mut self, id: u32, res_id: u32, owner: u32, owner_res: u32) -> i32 {
        if self.clients.get(&id).is_none_or(|c| c.resources.contains_key(&res_id)) {
            return 0;
        }
        let Some(tt) = self.clients.get(&owner).and_then(|c| c.resources.get(&owner_res)).and_then(|o| o.tt) else {
            Line::new().s("native: no res_id ").dec(owner_res as u64).s(" of client ").dec(owner as u64)
                .s(" to attach").say();
            return -EINVAL;
        };
        let cl = self.clients.get_mut(&id).expect("checked above");
        let Some(pages) = tt_pages(tt, tt_page_count(tt)) else { return -ENOMEM };
        let va = npk_sys::v3d_ctx_map(cl.ctx, &pages);
        if va <= 0 {
            return -ENOMEM;
        }
        cl.resources.insert(res_id, Obj { tt: None, va: va as u32, last: 0 });
        0
    }

    /// Finished submissions reported on the clients' rings, `WAIT_BO`s
    /// answered whose object is idle or whose time is up.
    pub fn poll(&mut self, c: &Composer) {
        for (&id, cl) in self.clients.iter_mut() {
            let before = cl.completed;
            while let Some(&(n, f)) = cl.inflight.front() {
                let (done, failed) = c.fence(f);
                if !done {
                    break;
                }
                if failed {
                    cl.async_error = cl.async_error.wrapping_add(1);
                    cl.resets += 1;
                }
                cl.completed = n;
                cl.inflight.pop_front();
            }
            if cl.completed != before {
                npk_sys::gpudev_ring(id as i32, RING, cl.submitted as i64, cl.completed as i64);
            }
        }
        let now = npk_sys::now_us() as u64;
        let mut i = 0;
        while i < self.waits.len() {
            let w = &self.waits[i];
            let done = c.fence(w.fence).0;
            if !done && now < w.until_us {
                i += 1;
                continue;
            }
            let w = self.waits.swap_remove(i);
            let ae = self.clients.get(&w.client).map_or(0, |c| c.async_error);
            let r = reply(ae, false, &rsp(if done { 0 } else { -ETIME }, &[]));
            let st = if npk_sys::gpudev_reply(w.seq, &r) != 0 { -ENOMEM } else { 0 };
            npk_sys::gpudev_complete(w.seq, st);
        }
    }

    /// The earliest `WAIT_BO` deadline (us), for the loop's timeout.
    pub fn next_due(&self) -> Option<u64> {
        self.waits.iter().map(|w| w.until_us).min()
    }
}

/// The pages of GTT pages `tt` (`npk_gpudev_tt_buffer`'s buffer size).
fn tt_page_count(tt: i32) -> u64 {
    let buf = npk_sys::gpudev_tt_buffer(tt);
    let mut o = [0u8; 40];
    if buf <= 0 || npk_sys::gpudev_bo_info(buf, &mut o) < 0 {
        return 0;
    }
    u32::from_le_bytes([o[36], o[37], o[38], o[39]]) as u64
}

/// `v3d_submit_cl_ioctl`, `_tfu`, `_csd`: the job in the client's
/// context, after its objects are looked up (`v3d_lookup_bos`). A refused
/// one counts in `async_error` and completes at once, so the guest's fence
/// still signals.
fn submit(c: &mut Composer, hw: &mut PiHw, cl: &mut Client, id: u32, cmd: u32, req: &[u8]) {
    let a = |i: usize| d32(req, REQ_HEAD + 4 * i);
    let res_at = |first: usize, n: u32| -> Option<Vec<u32>> {
        if n > MAX_SUBMIT_RES || req.len() < first + 4 * n as usize {
            return None;
        }
        Some((0..n as usize).map(|i| d32(req, first + 4 * i)).collect())
    };
    let (job, res) = match cmd {
        CCMD_SUBMIT_CL => {
            let (bcl, rcl) = ((a(0), a(1)), (a(2), a(3)));
            // A bin job only when there is a binning list (`v3d_submit_cl_ioctl`).
            let bin = (bcl.0 != bcl.1).then_some(BinJob { start: bcl.0, end: bcl.1, qma: a(4), qms: a(5), qts: a(6) });
            (Some(ClientJob::Cl { bin, render: rcl, flush: a(7) & SUBMIT_FLUSH_CACHE != 0 }),
             res_at(REQ_HEAD + 40, a(8)))
        }
        CCMD_SUBMIT_TFU => {
            let t = TfuJob { icfg: a(0), iia: a(1), iis: a(2), ica: a(3), iua: a(4), ioa: a(5), ios: a(6),
                             coef: [a(7), a(8), a(9), a(10)], ioc: a(11) };
            (Some(ClientJob::Tfu(t)), Some((12..16).map(a).filter(|&r| r != 0).collect()))
        }
        _ => {
            let cfg = core::array::from_fn(|i| a(i));
            (Some(ClientJob::Csd { cfg, flush: a(11) & SUBMIT_FLUSH_CACHE != 0 }), res_at(REQ_HEAD + 56, a(12)))
        }
    };
    cl.submitted += 1;
    let n = cl.submitted;
    let known = res.as_ref().is_some_and(|r| r.iter().all(|id| cl.resources.contains_key(id)));
    let fence = match (job, known) {
        (Some(job), true) => c.submit_client(hw, cl.ctx, job).ok(),
        _ => None,
    };
    match fence {
        Some(f) => {
            for r in res.unwrap_or_default() {
                if let Some(o) = cl.resources.get_mut(&r) {
                    o.last = f;
                }
            }
            cl.inflight.push_back((n, f));
        }
        None => {
            cl.async_error = cl.async_error.wrapping_add(1);
            if cl.inflight.is_empty() {
                cl.completed = n;
            } else {
                // Behind work still running: done with it.
                let last = cl.inflight.back().expect("not empty").1;
                cl.inflight.push_back((n, last));
            }
        }
    }
    npk_sys::gpudev_ring(id as i32, RING, cl.submitted as i64, cl.completed as i64);
}
