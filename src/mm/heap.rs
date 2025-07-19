use core::alloc::{GlobalAlloc, Layout};
use core::ptr;
use spin::Mutex;
use crate::{debug_info, debug_debug};

pub const HEAP_START: usize = 0x_4444_4444_0000;
pub const HEAP_SIZE: usize = 100 * 1024 * 1024; // 100 MiB

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub struct LockedHeap(Mutex<Option<linked_list_allocator::Heap>>);

impl LockedHeap {
    pub const fn empty() -> Self {
        LockedHeap(Mutex::new(None))
    }

    pub unsafe fn init(&self, heap_start: usize, heap_size: usize) {
        debug_info!("Initializing heap allocator at 0x{:x} with size {} MiB", 
            heap_start, heap_size / (1024 * 1024));
            
        let mut heap = linked_list_allocator::Heap::empty();
        heap.init(heap_start as *mut u8, heap_size);
        *self.0.lock() = Some(heap);
        
        debug_info!("Heap allocator initialized successfully");
    }
}

unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut heap = self.0.lock();
        
        match heap.as_mut() {
            Some(heap) => {
                match heap.allocate_first_fit(layout) {
                    Ok(non_null) => {
                        let ptr = non_null.as_ptr();
                        debug_debug!("Allocated {} bytes at {:p}", layout.size(), ptr);
                        ptr
                    }
                    Err(_) => {
                        debug_info!("Allocation failed for {} bytes", layout.size());
                        ptr::null_mut()
                    }
                }
            }
            None => ptr::null_mut(),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let mut heap = self.0.lock();
        
        if let Some(heap) = heap.as_mut() {
            debug_debug!("Deallocating {} bytes at {:p}", layout.size(), ptr);
            heap.deallocate(ptr::NonNull::new_unchecked(ptr), layout);
        }
    }
}

pub fn init_heap(
    mapper: &mut super::paging::MemoryMapper,
) -> Result<(), &'static str> {
    debug_info!("Initializing heap memory");
    
    // Don't pre-map the heap pages - let them be mapped on demand via page faults
    // This is more memory efficient and demonstrates demand paging
    
    unsafe {
        ALLOCATOR.init(HEAP_START, HEAP_SIZE);
    }
    
    Ok(())
}

#[alloc_error_handler]
fn alloc_error_handler(layout: Layout) -> ! {
    panic!("Allocation error: {:?}", layout)
}