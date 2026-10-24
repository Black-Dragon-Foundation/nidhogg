//! Copy: an opaque layer at full opacity, or any layer without chrome.

use super::row_kernel;

#[inline(always)]
fn copy_row(d: &mut [u32], s: &[u32]) {
    d.copy_from_slice(s);
}

row_kernel!(copy_base, copy_avx2, copy_row, (d: &mut [u32], s: &[u32]));
