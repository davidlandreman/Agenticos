//! Atomic Reference Counting (Arc) implementation for the kernel
//! 
//! This module provides a thread-safe reference-counted smart pointer similar
//! to std::sync::Arc, but designed for use in a no_std kernel environment.

use core::sync::atomic::{AtomicUsize, Ordering};
use core::ops::Deref;
use core::ptr::NonNull;
use core::marker::PhantomData;
use core::mem;
use core::fmt;
use alloc::alloc::{alloc, dealloc, Layout};

/// A thread-safe reference-counting pointer. 'Arc' stands for 'Atomically
/// Reference Counted'.
/// 
/// The type `Arc<T>` provides shared ownership of a value of type `T`,
/// allocated in the heap. Invoking `clone` on `Arc` produces a new `Arc`
/// instance, which points to the same allocation on the heap as the source
/// `Arc`, while increasing a reference count. When the last `Arc` pointer to
/// a given allocation is destroyed, the value stored in that allocation is
/// also dropped.
pub struct Arc<T: ?Sized> {
    ptr: NonNull<ArcInner<T>>,
    phantom: PhantomData<ArcInner<T>>,
}

/// The inner structure that holds the reference counts and the data
struct ArcInner<T: ?Sized> {
    strong: AtomicUsize,
    weak: AtomicUsize,
    data: T,
}

/// A weak reference to an `Arc<T>`.
/// 
/// Weak references do not keep the value alive and can be upgraded to
/// a strong reference if the value still exists.
pub struct Weak<T: ?Sized> {
    ptr: NonNull<ArcInner<T>>,
    phantom: PhantomData<ArcInner<T>>,
}

unsafe impl<T: ?Sized + Sync + Send> Send for Arc<T> {}
unsafe impl<T: ?Sized + Sync + Send> Sync for Arc<T> {}
unsafe impl<T: ?Sized + Sync + Send> Send for Weak<T> {}
unsafe impl<T: ?Sized + Sync + Send> Sync for Weak<T> {}

impl<T> Arc<T> {
    /// Constructs a new `Arc<T>`.
    pub fn new(data: T) -> Arc<T> {
        let layout = Layout::new::<ArcInner<T>>();
        let ptr = unsafe {
            let ptr = alloc(layout) as *mut ArcInner<T>;
            if ptr.is_null() {
                panic!("Failed to allocate memory for Arc");
            }
            
            // Initialize the ArcInner
            ptr.write(ArcInner {
                strong: AtomicUsize::new(1),
                weak: AtomicUsize::new(1), // Arc itself holds a weak reference
                data,
            });
            
            NonNull::new_unchecked(ptr)
        };
        
        Arc {
            ptr,
            phantom: PhantomData,
        }
    }
    
    /// Gets the number of strong (`Arc`) pointers to this allocation.
    pub fn strong_count(this: &Self) -> usize {
        this.inner().strong.load(Ordering::Acquire)
    }
    
    /// Gets the number of weak (`Weak`) pointers to this allocation.
    pub fn weak_count(this: &Self) -> usize {
        this.inner().weak.load(Ordering::Acquire) - 1
    }
    
    /// Returns a mutable reference into the given `Arc`, if there are
    /// no other `Arc` or `Weak` pointers to the same allocation.
    pub fn get_mut(this: &mut Self) -> Option<&mut T> {
        if this.is_unique() {
            unsafe {
                Some(&mut (*this.ptr.as_ptr()).data)
            }
        } else {
            None
        }
    }
    
    /// Consumes the `Arc`, returning the wrapped value if this was the
    /// last remaining reference.
    pub fn try_unwrap(this: Self) -> Result<T, Self> {
        if this.is_unique() {
            unsafe {
                let ptr = this.ptr.as_ptr();
                let data = core::ptr::read(&(*ptr).data);
                
                // Prevent the destructor from running
                mem::forget(this);
                
                // Deallocate the ArcInner
                let layout = Layout::new::<ArcInner<T>>();
                dealloc(ptr as *mut u8, layout);
                
                Ok(data)
            }
        } else {
            Err(this)
        }
    }
    
    /// Creates a new `Weak` pointer to this allocation.
    pub fn downgrade(this: &Self) -> Weak<T> {
        this.inner().weak.fetch_add(1, Ordering::Relaxed);
        Weak {
            ptr: this.ptr,
            phantom: PhantomData,
        }
    }
}

impl<T: ?Sized> Arc<T> {
    fn inner(&self) -> &ArcInner<T> {
        unsafe { self.ptr.as_ref() }
    }
    
    fn is_unique(&self) -> bool {
        self.inner().strong.load(Ordering::Acquire) == 1 &&
        self.inner().weak.load(Ordering::Acquire) == 1
    }
}

impl<T: ?Sized> Clone for Arc<T> {
    fn clone(&self) -> Arc<T> {
        let old_size = self.inner().strong.fetch_add(1, Ordering::Relaxed);
        
        // Check for overflow
        if old_size > isize::MAX as usize {
            panic!("Arc reference count overflow");
        }
        
        Arc {
            ptr: self.ptr,
            phantom: PhantomData,
        }
    }
}

impl<T: ?Sized> Deref for Arc<T> {
    type Target = T;
    
    fn deref(&self) -> &T {
        &self.inner().data
    }
}

impl<T: ?Sized> Drop for Arc<T> {
    fn drop(&mut self) {
        if self.inner().strong.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        
        // This is the last strong reference
        core::sync::atomic::fence(Ordering::Acquire);
        
        unsafe {
            // Drop the data
            core::ptr::drop_in_place(&mut (*self.ptr.as_ptr()).data);
            
            // If there are no weak references, deallocate
            if self.inner().weak.fetch_sub(1, Ordering::Release) == 1 {
                core::sync::atomic::fence(Ordering::Acquire);
                let layout = Layout::for_value(self.ptr.as_ref());
                dealloc(self.ptr.as_ptr() as *mut u8, layout);
            }
        }
    }
}

// Sentinel value for unallocated weak pointers
const WEAK_SENTINEL: usize = 1;

impl<T> Weak<T> {
    /// Constructs a new `Weak<T>` without allocating any memory.
    pub fn new() -> Weak<T> {
        Weak {
            ptr: NonNull::new(WEAK_SENTINEL as *mut ArcInner<T>).unwrap(),
            phantom: PhantomData,
        }
    }
    
    /// Attempts to upgrade the `Weak` pointer to an `Arc`.
    pub fn upgrade(&self) -> Option<Arc<T>> {
        if self.ptr.as_ptr() as *const u8 as usize == WEAK_SENTINEL {
            return None;
        }
        
        let inner = unsafe { self.ptr.as_ref() };
        
        // Try to increment the strong count
        let mut strong = inner.strong.load(Ordering::Relaxed);
        loop {
            if strong == 0 {
                return None;
            }
            
            match inner.strong.compare_exchange_weak(
                strong,
                strong + 1,
                Ordering::Relaxed,
                Ordering::Relaxed
            ) {
                Ok(_) => return Some(Arc {
                    ptr: self.ptr,
                    phantom: PhantomData,
                }),
                Err(new_strong) => strong = new_strong,
            }
        }
    }
}

impl<T: ?Sized> Weak<T> {
    /// Gets the number of strong (`Arc`) pointers to this allocation.
    pub fn strong_count(&self) -> usize {
        if self.ptr.as_ptr() as *const u8 as usize == WEAK_SENTINEL {
            0
        } else {
            unsafe { self.ptr.as_ref().strong.load(Ordering::Acquire) }
        }
    }
    
    /// Gets the number of weak (`Weak`) pointers to this allocation.
    pub fn weak_count(&self) -> usize {
        if self.ptr.as_ptr() as *const u8 as usize == WEAK_SENTINEL {
            0
        } else {
            unsafe { self.ptr.as_ref().weak.load(Ordering::Acquire) - 1 }
        }
    }
}

impl<T: ?Sized> Clone for Weak<T> {
    fn clone(&self) -> Weak<T> {
        if self.ptr.as_ptr() as *const u8 as usize != WEAK_SENTINEL {
            unsafe {
                self.ptr.as_ref().weak.fetch_add(1, Ordering::Relaxed);
            }
        }
        
        Weak {
            ptr: self.ptr,
            phantom: PhantomData,
        }
    }
}

impl<T> Default for Weak<T> {
    fn default() -> Self {
        Weak::new()
    }
}

impl<T: ?Sized> Drop for Weak<T> {
    fn drop(&mut self) {
        if self.ptr.as_ptr() as *const u8 as usize == WEAK_SENTINEL {
            return;
        }
        
        if unsafe { self.ptr.as_ref().weak.fetch_sub(1, Ordering::Release) } == 1 {
            core::sync::atomic::fence(Ordering::Acquire);
            unsafe {
                let layout = Layout::for_value(self.ptr.as_ref());
                dealloc(self.ptr.as_ptr() as *mut u8, layout);
            }
        }
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Arc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for Arc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: ?Sized> fmt::Pointer for Arc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&self.ptr.as_ptr(), f)
    }
}

impl<T: Default> Default for Arc<T> {
    fn default() -> Arc<T> {
        Arc::new(Default::default())
    }
}

impl<T: ?Sized + PartialEq> PartialEq for Arc<T> {
    fn eq(&self, other: &Arc<T>) -> bool {
        *(*self) == *(*other)
    }
}

impl<T: ?Sized + Eq> Eq for Arc<T> {}

impl<T: ?Sized + PartialOrd> PartialOrd for Arc<T> {
    fn partial_cmp(&self, other: &Arc<T>) -> Option<core::cmp::Ordering> {
        (**self).partial_cmp(&**other)
    }
}

impl<T: ?Sized + Ord> Ord for Arc<T> {
    fn cmp(&self, other: &Arc<T>) -> core::cmp::Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: ?Sized + core::hash::Hash> core::hash::Hash for Arc<T> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        (**self).hash(state);
    }
}