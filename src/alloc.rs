//! Maps large blocks straight from the kernel so freeing them gives the memory back.
//!
//! macOS 26 malloc keeps freed large blocks resident (`MALLOC_LARGE (empty)` in vmmap), and
//! `malloc_zone_pressure_relief` doesn't release them: after fitting one 48MP photo the
//! process held 206 MB of decode buffers it would never touch again.

use std::alloc::{GlobalAlloc, Layout, System};
use std::ptr;

/// Image buffers and decoder scratch space are far above this; ordinary allocations are below.
const DIRECT_MIN: usize = 1 << 20;
/// mmap only guarantees the base page size (4 KiB on Intel Macs).
const PAGE: usize = 4096;

pub struct Allocator;

fn is_direct(size: usize, align: usize) -> bool {
    size >= DIRECT_MIN && align <= PAGE
}

unsafe fn map(size: usize) -> *mut u8 {
    // SAFETY: an anonymous private mapping has no preconditions; failure is checked below.
    let p = unsafe {
        libc::mmap(
            ptr::null_mut(),
            size,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    if p == libc::MAP_FAILED {
        ptr::null_mut()
    } else {
        p.cast()
    }
}

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if is_direct(layout.size(), layout.align()) {
            unsafe { map(layout.size()) }
        } else {
            unsafe { System.alloc(layout) }
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if is_direct(layout.size(), layout.align()) {
            // Fresh anonymous pages are already zero.
            unsafe { map(layout.size()) }
        } else {
            unsafe { System.alloc_zeroed(layout) }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if is_direct(layout.size(), layout.align()) {
            // SAFETY: `ptr` came from `map` with this same size.
            unsafe { libc::munmap(ptr.cast(), layout.size()) };
        } else {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let align = layout.align();
        if !is_direct(layout.size(), align) && !is_direct(new_size, align) {
            return unsafe { System.realloc(ptr, layout, new_size) };
        }
        // macOS has no mremap: move the bytes. Only growing Vecs above 1 MB get here.
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, align) };
        let new = unsafe { self.alloc(new_layout) };
        if !new.is_null() {
            unsafe {
                ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        new
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_blocks_round_trip_through_realloc() {
        let a = Allocator;
        unsafe {
            let small = Layout::from_size_align(1000, 8).unwrap();
            let p = a.alloc(small);
            p.write_bytes(7, 1000);
            // small → direct
            let p = a.realloc(p, small, 3 << 20);
            assert_eq!(*p.add(999), 7);
            p.add(3 << 20).sub(1).write(9);
            // direct → direct
            let big = Layout::from_size_align(3 << 20, 8).unwrap();
            let p = a.realloc(p, big, 5 << 20);
            assert_eq!((*p.add(999), *p.add((3 << 20) - 1)), (7, 9));
            // direct → small
            let bigger = Layout::from_size_align(5 << 20, 8).unwrap();
            let p = a.realloc(p, bigger, 500);
            assert_eq!(*p.add(499), 7);
            a.dealloc(p, Layout::from_size_align(500, 8).unwrap());
        }
    }

    #[test]
    fn zeroed_large_block_is_zero() {
        let a = Allocator;
        unsafe {
            let layout = Layout::from_size_align(2 << 20, 64).unwrap();
            let p = a.alloc_zeroed(layout);
            assert!((0..layout.size()).step_by(4096).all(|i| *p.add(i) == 0));
            a.dealloc(p, layout);
        }
    }
}
