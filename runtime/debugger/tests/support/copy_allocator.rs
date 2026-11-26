//! Thread-local, one-shot allocation failure for a selected copy buffer size.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};
thread_local! {
    static SIZE: Cell<Option<usize>> = const { Cell::new(None) };
    static FIRED: Cell<bool> = const { Cell::new(false) };
}
struct CopyAllocator;
#[global_allocator]
static ALLOCATOR: CopyAllocator = CopyAllocator;
fn fail(size: usize) -> bool {
    SIZE.try_with(|target| {
        if target.get() == Some(size) {
            target.set(None);
            FIRED.with(|fired| fired.set(true));
            true
        } else {
            false
        }
    })
    .unwrap_or(false)
}
// SAFETY: Successful operations delegate unchanged layouts/pointers to System.
// A selected allocation returns null once, as GlobalAlloc permits, without
// touching a failed realloc's input. TLS checks allocate nothing and never unwind.
unsafe impl GlobalAlloc for CopyAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if fail(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: The caller supplies a valid allocation layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if fail(layout.size()) {
            return std::ptr::null_mut();
        }
        // SAFETY: The caller supplies a valid allocation layout.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if fail(size) {
            return std::ptr::null_mut();
        }
        // SAFETY: The caller supplies the live input allocation and its layout.
        unsafe { System.realloc(ptr, layout, size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: Every successful allocation originated from System.
        unsafe { System.dealloc(ptr, layout) }
    }
}
struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        SIZE.with(|size| size.set(None));
    }
}
pub fn fail_size<T>(size: usize, body: impl FnOnce() -> T) -> (T, bool) {
    SIZE.with(|target| {
        assert!(target.get().is_none());
        target.set(Some(size));
    });
    FIRED.with(|fired| fired.set(false));
    let guard = Reset;
    let result = body();
    drop(guard);
    (result, FIRED.with(Cell::get))
}
