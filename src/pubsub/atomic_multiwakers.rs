use core::{cell::RefCell, ops::Deref, task::Waker};

use embassy_sync::blocking_mutex::{
    raw::{CriticalSectionRawMutex, RawMutex},
    Mutex,
};
use heapless::Vec;

/// Utility struct to register and wake multiple wakers.
pub struct MultiWakerRegistration<const N: usize> {
    wakers: Mutex<CriticalSectionRawMutex, RefCell<Vec<Waker, N>>>,
}

impl<const N: usize> MultiWakerRegistration<N> {
    /// Create a new empty instance
    pub const fn new() -> Self {
        Self {
            wakers: Mutex::const_new(CriticalSectionRawMutex::INIT, RefCell::new(Vec::new())),
        }
    }

    /// Register a waker.
    pub fn register(&self, w: &Waker) {
        self.wakers.lock(|inner| {
            let mut wakers = inner.borrow_mut();

            // If we already have some waker that wakes the same task as `w`, do nothing.
            // This avoids cloning wakers, and avoids unnecessary mass-wakes.
            for w2 in wakers.deref() {
                if w.will_wake(w2) {
                    return;
                }
            }

            if wakers.is_full() {
                // All waker slots were full. It's a bit inefficient, but we can wake everything.
                // Any future that is still active will simply reregister.
                // This won't happen a lot, so it's ok.
                self.wake();
            }

            if wakers.push(w.clone()).is_err() {
                // This can't happen unless N=0
                // (Either `wakers` wasn't full, or it was in which case `wake()` empied it)
                panic!("tried to push a waker to a zero-length MultiWakerRegistration")
            }
        })
    }

    /// Wake all registered wakers. This clears the buffer
    pub fn wake(&self) {
        self.wakers.lock(|inner| {
            let mut wakers = inner.borrow_mut();

            // heapless::Vec has no `drain()`, do it unsafely ourselves...

            // First set length to 0, without dropping the contents.
            // This is necessary for soundness: if wake() panics and we're using panic=unwind.
            // Setting len=0 upfront ensures other code can't observe the vec in an inconsistent state.
            // (it'll leak wakers, but that's not UB)
            let len = wakers.len();
            unsafe { wakers.set_len(0) }

            for i in 0..len {
                // Move a waker out of the vec.
                let waker = unsafe { wakers.as_mut_ptr().add(i).read() };
                // Wake it by value, which consumes (drops) it.
                waker.wake();
            }
        })
    }
}
