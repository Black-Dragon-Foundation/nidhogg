//! One `wl_shm` buffer: a sealed memfd with a page-aligned size, mapped for the host to draw into, with a
//! `wl_shm_pool` made for it alone (destroyed right after its buffer exists) and the `wl_buffer` on the reader queue,
//! so `release` runs on the reader thread.

use crate::wayland::MainState;
use crate::wayland::present::BufData;
use crate::wayland::reader::ReaderState;
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, ftruncate, memfd_create};
use rustix::mm::{MapFlags, ProtFlags, mmap, munmap};
use std::os::fd::AsFd;
use wayland_client::QueueHandle;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_shm::{Format, WlShm};

pub struct ShmBuf {
    pub buffer: WlBuffer,
    pub w: i32,
    pub h: i32,
    pub stride: i32,
    ptr: *mut u8,
    /// The mapping's length, and the memfd's size: a multiple of the page size.
    pub len: usize,
}

// SAFETY: the mapping is plain shared memory; the host writes it only between `present_begin` and
// `present_commit`, and the pointer is never used from the reader thread.
unsafe impl Send for ShmBuf {}

fn page_size() -> usize {
    // SAFETY: sysconf has no preconditions.
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 { n as usize } else { 4096 }
}

impl ShmBuf {
    /// A buffer of `w` x `h` ARGB8888 pixels.  `stride` is a multiple of 256 bytes.  `InvalidInput` when the buffer
    /// would not fit the protocol's pool size (an int32: under 2 GiB).
    pub fn create(
        shm: &WlShm,
        qh: &QueueHandle<MainState>,
        rqh: &QueueHandle<ReaderState>,
        w: i32,
        h: i32,
        data: BufData,
    ) -> std::io::Result<ShmBuf> {
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
        })
    }

    pub fn pixels(&self) -> *mut u8 {
        self.ptr
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
