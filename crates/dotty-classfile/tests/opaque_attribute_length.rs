#![cfg(all(target_os = "linux", target_pointer_width = "64"))]

use dotty_classfile::classfile::{
    Attribute, ConstantPool, ConstantPoolEntry, ConstantPoolIndex, EncodeError, RawAttribute,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::c_void;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

const LARGE_ALLOCATION_LIMIT: usize = 1024 * 1024;
const LIMIT_LARGE_ALLOCATIONS_ENV: &str = "DOTTY_CLASSFILE_LIMIT_LARGE_TEST_ALLOCATIONS";

static LIMIT_LARGE_ALLOCATIONS: AtomicBool = AtomicBool::new(false);

struct LimitedAllocator;

// The allocator is enabled only in the child process running the focused test.
unsafe impl GlobalAlloc for LimitedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if LIMIT_LARGE_ALLOCATIONS.load(Ordering::Relaxed) && layout.size() > LARGE_ALLOCATION_LIMIT
        {
            return std::ptr::null_mut();
        }
        // SAFETY: Delegate the unchanged layout to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: The pointer was allocated by the system allocator above.
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if LIMIT_LARGE_ALLOCATIONS.load(Ordering::Relaxed) && new_size > LARGE_ALLOCATION_LIMIT {
            return std::ptr::null_mut();
        }
        // SAFETY: Delegate the original allocation and requested size to the system allocator.
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: LimitedAllocator = LimitedAllocator;

unsafe extern "C" {
    fn mmap(
        address: *mut c_void,
        length: usize,
        protection: i32,
        flags: i32,
        file_descriptor: i32,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(address: *mut c_void, length: usize) -> i32;
}

#[test]
fn opaque_attribute_payload_overflow_returns_before_allocating_or_copying() {
    let limit_allocations_in_this_process = std::env::var_os(LIMIT_LARGE_ALLOCATIONS_ENV).is_some();
    if !limit_allocations_in_this_process {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "opaque_attribute_payload_overflow_returns_before_allocating_or_copying",
            ])
            .env(LIMIT_LARGE_ALLOCATIONS_ENV, "1")
            .status()
            .unwrap();
        assert!(
            status.success(),
            "bounded-allocation child failed: {status}"
        );
        return;
    }

    let payload_length = (u32::MAX as usize) + 1;
    // Map read-only virtual zero pages so the test can supply the public
    // encoder with a >u4 borrowed slice without touching or allocating 4 GiB.
    // The child allocator also refuses any allocation over 1 MiB.
    const PROT_READ: i32 = 0x1;
    const MAP_PRIVATE: i32 = 0x02;
    const MAP_ANONYMOUS: i32 = 0x20;
    const MAP_NORESERVE: i32 = 0x4000;
    // SAFETY: The mapping is anonymous and the requested range remains mapped
    // until it is explicitly released below.
    let mapped = unsafe {
        mmap(
            std::ptr::null_mut(),
            payload_length,
            PROT_READ,
            MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE,
            -1,
            0,
        )
    };
    assert_ne!(mapped, (-1isize) as *mut c_void, "mmap failed");

    // SAFETY: `mapped` names a valid, readable anonymous mapping of this exact
    // length; the encoder must reject the length before reading its contents.
    let bytes = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), payload_length) };
    let pool = ConstantPool::from_entries(vec![Some(ConstantPoolEntry::Utf8(
        "StackMapTable".to_owned(),
    ))]);
    LIMIT_LARGE_ALLOCATIONS.store(true, Ordering::Relaxed);
    let result = Attribute::Other(RawAttribute {
        name_index: ConstantPoolIndex(1),
        bytes,
    })
    .encode(&pool);
    LIMIT_LARGE_ALLOCATIONS.store(false, Ordering::Relaxed);

    // SAFETY: Release the full mapping created above after the encoder returns.
    assert_eq!(unsafe { munmap(mapped, payload_length) }, 0);
    assert_eq!(
        result,
        Err(EncodeError::CountOverflow {
            value: payload_length,
            width: dotty_classfile::classfile::LengthWidth::U32,
        })
    );
}
