use spin::Mutex;
use lazy_static::lazy_static;
use alloc::collections::VecDeque;

/// Simple waker system for signaling blocked operations
pub struct Waker {
    woken: bool,
}

impl Waker {
    pub const fn new() -> Self {
        Self { woken: false }
    }
    
    /// Wake up anyone waiting on this waker
    pub fn wake(&mut self) {
        self.woken = true;
    }
    
    /// Check if we've been woken and reset the flag
    pub fn poll_and_reset(&mut self) -> bool {
        let was_woken = self.woken;
        self.woken = false;
        was_woken
    }
    
    /// Check if we've been woken without resetting
    pub fn is_woken(&self) -> bool {
        self.woken
    }
}

/// Global waker registry for stdin events
pub struct StdinWakerRegistry {
    wakers: VecDeque<*mut Waker>,
}

unsafe impl Send for StdinWakerRegistry {}
unsafe impl Sync for StdinWakerRegistry {}

impl StdinWakerRegistry {
    const fn new() -> Self {
        Self {
            wakers: VecDeque::new(),
        }
    }
    
    /// Register a waker to be notified of stdin events
    /// SAFETY: The waker must remain valid until unregistered
    pub unsafe fn register(&mut self, waker: *mut Waker) {
        self.wakers.push_back(waker);
    }
    
    /// Unregister a waker
    pub fn unregister(&mut self, waker: *mut Waker) {
        self.wakers.retain(|&w| w != waker);
    }
    
    /// Wake all registered stdin wakers
    /// This should be called from keyboard interrupt context
    pub fn wake_all(&mut self) {
        for &waker_ptr in &self.wakers {
            unsafe {
                if !waker_ptr.is_null() {
                    (*waker_ptr).wake();
                }
            }
        }
    }
}

lazy_static! {
    static ref STDIN_WAKER_REGISTRY: Mutex<StdinWakerRegistry> = 
        Mutex::new(StdinWakerRegistry::new());
}

/// Register a waker for stdin events
/// SAFETY: The waker must remain valid until unregistered
pub unsafe fn register_stdin_waker(waker: *mut Waker) {
    STDIN_WAKER_REGISTRY.lock().register(waker);
}

/// Unregister a stdin waker
pub fn unregister_stdin_waker(waker: *mut Waker) {
    STDIN_WAKER_REGISTRY.lock().unregister(waker);
}

/// Wake all stdin waiters - called from keyboard interrupt
pub fn wake_stdin_waiters() {
    STDIN_WAKER_REGISTRY.lock().wake_all();
}