use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use boxt_core::message::WORKER_MEMORY_CAP_EXIT;

pub struct CapAllocator;

static IN_WORKER: AtomicBool = AtomicBool::new(false);

pub(crate) fn enter_worker() {
    IN_WORKER.store(true, Ordering::SeqCst);
}

#[cfg(not(windows))]
fn allocation_failed() -> ! {
    std::process::exit(WORKER_MEMORY_CAP_EXIT)
}

/// `exit` would run the C runtime's exit path and DLL detach code, which allocate; under the
/// Job Object's memory limit those allocations fail and raise an exception, which reads as a
/// crash. Terminating skips all of it.
#[cfg(windows)]
fn allocation_failed() -> ! {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    unsafe {
        TerminateProcess(GetCurrentProcess(), WORKER_MEMORY_CAP_EXIT as u32);
    }
    std::process::abort()
}

fn checked(ptr: *mut u8) -> *mut u8 {
    if ptr.is_null() && IN_WORKER.load(Ordering::Relaxed) {
        allocation_failed();
    }
    ptr
}

unsafe impl GlobalAlloc for CapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        checked(unsafe { System.alloc(layout) })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        checked(unsafe { System.alloc_zeroed(layout) })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        checked(unsafe { System.realloc(ptr, layout, new_size) })
    }
}

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
    fn calloc(count: usize, size: usize) -> *mut c_void;
    fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
}

unsafe extern "C" fn cap_malloc(size: usize) -> *mut c_void {
    let ptr = unsafe { malloc(size) };
    if ptr.is_null() && size != 0 {
        allocation_failed();
    }
    ptr
}

unsafe extern "C" fn cap_calloc(count: usize, size: usize) -> *mut c_void {
    let ptr = unsafe { calloc(count, size) };
    if ptr.is_null() && count != 0 && size != 0 {
        allocation_failed();
    }
    ptr
}

unsafe extern "C" fn cap_realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    let ptr = unsafe { realloc(ptr, size) };
    if ptr.is_null() && size != 0 {
        allocation_failed();
    }
    ptr
}

unsafe extern "C" fn cap_free(ptr: *mut c_void) {
    unsafe { free(ptr) }
}

pub(crate) fn install_parser_allocator() {
    unsafe { boxt_read_cpp::set_c_allocator(cap_malloc, cap_calloc, cap_realloc, cap_free) }
}
