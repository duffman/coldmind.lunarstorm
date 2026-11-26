//! Unit-test allocator fault injection. Only the armed calling thread is affected,
//! and exactly one allocation fails; error-report construction then uses System.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static FAILURE: Cell<Option<usize>> = const { Cell::new(None) };
    static FIRED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<Option<usize>> = const { Cell::new(None) };
}
struct TestAllocator;
#[global_allocator]
static ALLOCATOR: TestAllocator = TestAllocator;

fn fail() -> bool {
    let _ = COUNT.try_with(|count| {
        if let Some(value) = count.get() {
            count.set(Some(value.saturating_add(1)));
        }
    });
    FAILURE
        .try_with(|remaining| match remaining.get() {
            None => false,
            Some(0) => {
                remaining.set(None);
                FIRED.with(|fired| fired.set(true));
                true
            }
            Some(count) => {
                remaining.set(Some(count - 1));
                false
            }
        })
        .unwrap_or(false)
}
// SAFETY: Every successful operation delegates its unchanged pointer/layout to
// System. A selected alloc/realloc returns null, as GlobalAlloc permits; failed
// realloc leaves the original allocation untouched. TLS access allocates nothing
// and performs no user callback, and allocation methods never unwind.
unsafe impl GlobalAlloc for TestAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if fail() {
            return std::ptr::null_mut();
        }
        // SAFETY: The caller supplies a valid allocation layout.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if fail() {
            return std::ptr::null_mut();
        }
        // SAFETY: The caller supplies a valid allocation layout.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: Successful allocations all originate from System, and the
        // caller supplies the same live pointer/layout required by GlobalAlloc.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if fail() {
            return std::ptr::null_mut();
        }
        // SAFETY: The caller provides the original live layout and valid new size.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

struct Reset;
impl Drop for Reset {
    fn drop(&mut self) {
        FAILURE.with(|failure| failure.set(None));
    }
}

/// Fails allocation `index` (zero-based) made by `body`, then restores normal
/// allocation. The second result verifies the intended allocation was exercised.
pub(crate) fn fail_nth<T>(index: usize, body: impl FnOnce() -> T) -> (T, bool) {
    FAILURE.with(|failure| assert!(failure.get().is_none(), "allocator guards cannot nest"));
    FIRED.with(|fired| fired.set(false));
    FAILURE.with(|failure| failure.set(Some(index)));
    let reset = Reset;
    let result = body();
    drop(reset);
    (result, FIRED.with(Cell::get))
}

struct StopCounting;
impl Drop for StopCounting {
    fn drop(&mut self) {
        COUNT.with(|count| count.set(None));
    }
}

/// Counts allocation/reallocation requests on the current owner thread only.
/// Use the same allocator for both benchmark paths and warm both before arming.
pub(crate) fn count_allocations<T>(body: impl FnOnce() -> T) -> (T, usize) {
    COUNT.with(|count| {
        assert!(count.get().is_none(), "allocation counters cannot nest");
        count.set(Some(0));
    });
    let reset = StopCounting;
    let result = body();
    let count = COUNT.with(|count| count.get().unwrap());
    drop(reset);
    (result, count)
}

#[test]
fn counting_observes_real_growth_and_leaves_owned_bytes_intact() {
    let (bytes, count) = count_allocations(|| {
        let mut bytes = Vec::<u8>::with_capacity(4);
        bytes.extend([1, 2, 3, 4]);
        bytes.reserve_exact(4);
        bytes
    });
    assert_eq!(bytes, [1, 2, 3, 4]);
    assert_eq!(
        count, 2,
        "one allocation followed by one real growth request"
    );
}
