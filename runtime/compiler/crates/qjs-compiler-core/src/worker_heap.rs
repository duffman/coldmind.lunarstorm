//! Aggregate requested Rust heap bound for the disposable compiler process.
//! OXC arena allocations use the global allocator, so they share this bound
//! with source normalization, semantic analysis, owned IR and image encoding.
//! This is not a resident-memory measurement. No allocator is changed in hosts.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
};
static LIVE: AtomicUsize = AtomicUsize::new(0);
static LIMIT: AtomicUsize = AtomicUsize::new(qjs_compiler_core::job_protocol::MAX_HEAP_BYTES);
struct BudgetAllocator;
#[global_allocator]
static ALLOCATOR: BudgetAllocator = BudgetAllocator;

#[cold]
fn capacity() -> ! {
    // No formatting, IO, unwinding or allocator reentry on the exhaustion path.
    unsafe extern "C" {
        fn _exit(status: i32) -> !;
    }
    // SAFETY: terminate only this disposable worker with the capacity exit code.
    unsafe { _exit(2) }
}
fn reserve(bytes: usize) {
    if LIVE
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
            live.checked_add(bytes)
                .filter(|next| *next <= LIMIT.load(Ordering::Relaxed))
        })
        .is_err()
    {
        capacity()
    }
}
pub fn configure() -> Result<(), String> {
    let value = std::env::var("QJS_COMPILER_HEAP_BYTES").ok();
    let limit = match value.as_deref() {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| "invalid compiler heap limit")?,
        None => qjs_compiler_core::job_protocol::MAX_HEAP_BYTES,
    };
    if limit == 0 || limit > qjs_compiler_core::job_protocol::MAX_HEAP_BYTES {
        return Err("invalid compiler heap limit".into());
    }
    LIMIT.store(limit, Ordering::Relaxed);
    if LIVE.load(Ordering::Relaxed) > limit {
        capacity()
    }
    Ok(())
}
// SAFETY: every request is forwarded to System with its original layout; all
// successful allocations/resizes are counted and exactly released on dealloc.
// Exhaustion exits this isolated process and never returns an invalid pointer.
unsafe impl GlobalAlloc for BudgetAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        reserve(layout.size());
        // SAFETY: GlobalAlloc caller supplies a valid nonzero layout.
        let pointer = unsafe { System.alloc(layout) };
        if pointer.is_null() {
            capacity()
        }
        parser_allocation();
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        reserve(layout.size());
        // SAFETY: GlobalAlloc caller supplies a valid nonzero layout.
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if pointer.is_null() {
            capacity()
        }
        parser_allocation();
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: allocation and exact original layout come from this allocator.
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if size > layout.size() {
            reserve(size - layout.size())
        }
        // SAFETY: GlobalAlloc's caller guarantees valid pointer/layout/new size.
        let result = unsafe { System.realloc(pointer, layout, size) };
        if result.is_null() {
            capacity()
        }
        if size < layout.size() {
            LIVE.fetch_sub(layout.size() - size, Ordering::Relaxed);
        }
        parser_allocation();
        result
    }
}

// Worker-private observability. The first allocation after ParsePhase::Begin is
// observed inside OXC's call stack. The marker performs no allocation itself.
static PARSE_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PARSE_REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PROGRESS: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
pub fn set_progress(file: std::fs::File) {
    let _ = PROGRESS.set(file);
}
fn marker(byte: u8) {
    use std::io::Write;
    if let Some(mut file) = PROGRESS.get()
        && file.write_all(&[byte]).is_err()
    {
        std::process::abort();
    }
}
pub fn parse_phase(phase: qjs_frontend::ParsePhase) {
    match phase {
        qjs_frontend::ParsePhase::Begin => {
            PARSE_REPORTED.store(false, Ordering::Relaxed);
            PARSE_ACTIVE.store(true, Ordering::Relaxed);
        }
        qjs_frontend::ParsePhase::End => {
            PARSE_ACTIVE.store(false, Ordering::Relaxed);
            marker(b'E');
        }
    }
}
fn parser_allocation() {
    if PARSE_ACTIVE.load(Ordering::Relaxed) && !PARSE_REPORTED.swap(true, Ordering::Relaxed) {
        marker(b'P');
    }
}
