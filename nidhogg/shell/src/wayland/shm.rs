//! One `wl_shm` buffer: a sealed memfd with a page-aligned size, mapped for the host to draw into, with a
//! `wl_shm_pool` made for it alone (destroyed right after its buffer exists) and the `wl_buffer` on the reader queue,
//! so `release` runs on the reader thread.  A content layer's ring buffer keeps its memfd, so that rows far from the
//! view can give their pages back (`punch`).

use crate::wayland::MainState;
use crate::wayland::reader::ReaderState;
use rustix::fs::{
    FallocateFlags, MemfdFlags, SealFlags, fallocate, fcntl_add_seals, ftruncate, memfd_create,
};
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{Format, WlShm};
use wayland_client::{Dispatch, QueueHandle};

pub struct ShmBuf {
    pub buffer: WlBuffer,
    pub w: i32,
    pub h: i32,
    pub stride: i32,
    ptr: *mut u8,
    /// The mapping's length, and the memfd's size: a multiple of the page size.
    pub len: usize,
    /// The memfd, kept for a ring buffer only.
    fd: Option<OwnedFd>,
}

// SAFETY: the mapping is plain shared memory, written by one thread at a time: the host between `present_begin` and
// `present_commit`, or whoever presents between taking the buffer and committing it; the reader thread never uses
// the pointer.  A content layer's newest buffer is read while another of its buffers is written (`Sync`).
unsafe impl Send for ShmBuf {}
// SAFETY: as above; shared references only read the mapping or punch pages outside every source rectangle.
unsafe impl Sync for ShmBuf {}

fn page_size() -> usize {
    // SAFETY: sysconf has no preconditions.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as usize } else { 4096 }
}

impl ShmBuf {
    /// A buffer of `w` x `h` ARGB8888 pixels.  `stride` is a multiple of 256 bytes.  `InvalidInput` when the buffer
    /// would not fit the protocol's pool size (an int32: under 2 GiB).
    pub fn create<U: Send + Sync + 'static>(
        shm: &WlShm,
        qh: &QueueHandle<MainState>,
        rqh: &QueueHandle<ReaderState>,
        w: i32,
        h: i32,
        data: U,
    ) -> std::io::Result<ShmBuf>
    where
        ReaderState: Dispatch<WlBuffer, U>,
    {
        Self::make(shm, qh, rqh, w, h, data, false)
    }

    /// `create`, keeping the memfd for `punch`: a content layer's ring buffer.
    pub fn create_ring<U: Send + Sync + 'static>(
        shm: &WlShm,
        qh: &QueueHandle<MainState>,
        rqh: &QueueHandle<ReaderState>,
        w: i32,
        h: i32,
        data: U,
    ) -> std::io::Result<ShmBuf>
    where
        ReaderState: Dispatch<WlBuffer, U>,
    {
        Self::make(shm, qh, rqh, w, h, data, true)
    }

    fn make<U: Send + Sync + 'static>(
        shm: &WlShm,
        qh: &QueueHandle<MainState>,
        rqh: &QueueHandle<ReaderState>,
        w: i32,
        h: i32,
        data: U,
        keep_fd: bool,
    ) -> std::io::Result<ShmBuf>
    where
        ReaderState: Dispatch<WlBuffer, U>,
    {
        let bad = || std::io::Error::from(std::io::ErrorKind::InvalidInput);
        // `stride_for` computes 4 w + 255 in an i32
        if w <= 0 || h <= 0 || w > (i32::MAX - 255) / 4 {
            return Err(bad());
        }
        let stride = glitnir::pool::stride_for(w);
        let page = page_size();
        let len = (stride as usize)
            .checked_mul(h as usize)
            .map(|n| n.div_ceil(page) * page)
            .filter(|&n| i32::try_from(n).is_ok())
            .ok_or_else(bad)?;
        let fd = memfd_create(
            "nhshell-buffer",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?;
        ftruncate(&fd, len as u64)?;
        fcntl_add_seals(&fd, SealFlags::SHRINK | SealFlags::GROW)?;
        // SAFETY: a fresh shared mapping of the whole file; it is unmapped in `Drop`.
        let ptr = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                &fd,
                0,
            )?
        } as *mut u8;
        let pool = shm.create_pool(fd.as_fd(), len as i32, qh, ());
        let buffer = pool.create_buffer(0, w, h, stride, Format::Argb8888, rqh, data);
        pool.destroy();
        Ok(ShmBuf {
            buffer,
            w,
            h,
            stride,
            ptr,
            len,
            fd: keep_fd.then_some(fd),
        })
    }

    pub fn pixels(&self) -> *mut u8 {
        self.ptr
    }

    /// Give back the pages that lie wholly inside rows `from..to` (a ring buffer's; they read as zeros after).
    pub fn punch(&self, from: i32, to: i32) {
        let Some(fd) = &self.fd else { return };
        let page = page_size();
        let row = |r: i32| (r.clamp(0, self.h) as usize * self.stride as usize).min(self.len);
        let (start, end) = (row(from).next_multiple_of(page), row(to) / page * page);
        if end > start {
            let _ = fallocate(
                fd,
                FallocateFlags::PUNCH_HOLE | FallocateFlags::KEEP_SIZE,
                start as u64,
                (end - start) as u64,
            );
        }
    }

    /// The memfd's resident 512-byte blocks (a ring buffer's; 0 for another).
    pub fn blocks(&self) -> u64 {
        let Some(fd) = &self.fd else { return 0 };
        // SAFETY: `fd` is open, and `st` a zeroed, writable stat (all-zero is a valid value of it).
        unsafe {
            let mut st: libc::stat = std::mem::zeroed();
            if libc::fstat(fd.as_raw_fd(), &mut st) != 0 {
                return 0;
            }
            st.st_blocks as u64
        }
    }
}

impl Drop for ShmBuf {
    fn drop(&mut self) {
        self.buffer.destroy();
        // SAFETY: `ptr`/`len` are the mapping made in `create`, unmapped once.
        unsafe {
            let _ = munmap(self.ptr.cast(), self.len);
        }
    }
}
