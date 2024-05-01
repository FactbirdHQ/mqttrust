use core::{cell::UnsafeCell, sync::atomic::{AtomicBool, AtomicUsize}};

use embassy_sync::waitqueue::{AtomicWaker, MultiWakerRegistration};

use super::{BufferProvider, StaticBufferProvider};

pub(crate) struct PubSubState<B, const SUBS: usize>
where
    B: BufferProvider,
{
    /// The buffer provider
    pub(crate) buf: UnsafeCell<B>,

    /// Max capacity of the buffer
    pub(crate) capacity: usize,

    /// Where the next byte will be written
    pub(crate) write: AtomicUsize,

    /// Where the next byte will be read from
    pub(crate) read: AtomicUsize,

    /// Used in the inverted case to mark the end of the
    /// readable streak. Otherwise will == sizeof::<self.buf>().
    /// Writer is responsible for placing this at the correct
    /// place when entering an inverted condition, and Reader
    /// is responsible for moving it back to sizeof::<self.buf>()
    /// when exiting the inverted condition
    pub(crate) last: AtomicUsize,

    /// Used by the Writer to remember what bytes are currently
    /// allowed to be written to, but are not yet ready to be
    /// read from
    pub(crate) reserve: AtomicUsize,

    /// Is there an active read grant?
    pub(crate) read_in_progress: AtomicBool,

    /// Is there an active write grant?
    pub(crate) write_in_progress: AtomicBool,

    /// Collection of wakers for Subscribers that are waiting.  
    pub(crate) subscriber_wakers: MultiWakerRegistration<SUBS>,

    pub(crate) subscriber_count: AtomicUsize,
    pub(crate) publisher_taken: AtomicBool,

    /// Write waker for async support
    /// Woken up when a release is done
    pub(crate) publisher_waker: AtomicWaker,
}

unsafe impl<B, const SUBS: usize> Sync for PubSubState<B, SUBS> where B: BufferProvider {}

impl<B, const SUBS: usize> PubSubState<B, SUBS>
where
    B: BufferProvider,
{
    /// Create a new PubSubState with abstraction over the memory provider
    ///
    /// ```rust,no_run
    /// use PubSubState::{PubSubState, StaticBufferProvider};
    ///
    ///
    /// fn main() {
    ///    let provider = StaticBufferProvider::<6>::new();
    ///    let mut buf = PubSubState::new(provider);
    ///    let (prod, cons) = buf.try_split().unwrap();
    /// }
    /// ```
    pub fn new(mut buf: B) -> Self {
        Self {
            capacity: buf.buf().len(),
            buf: UnsafeCell::new(buf),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            last: AtomicUsize::new(0),
            reserve: AtomicUsize::new(0),
            read_in_progress: AtomicBool::new(false),
            write_in_progress: AtomicBool::new(false),
            publisher_taken: AtomicBool::new(false),
            publisher_waker: AtomicWaker::new(),
            subscriber_count: AtomicUsize::new(0),
            subscriber_wakers: MultiWakerRegistration::new(),
        }
    }

    /// Returns the size of the backing storage.
    ///
    /// This is the maximum number of bytes that can be stored in this queue.
    ///
    /// ```rust
    /// # // PubSubState test shim!
    /// # fn bbqtest() {
    /// use PubSubState::{PubSubState, StaticBufferProvider};
    ///
    /// // Create a new buffer of 6 elements
    /// let mut buffer: PubSubState<StaticBufferProvider<6>> = PubSubState::new_static();
    /// assert_eq!(buffer.capacity(), 6);
    /// # // PubSubState test shim!
    /// # }
    /// #
    /// # fn main() {
    /// # #[cfg(not(feature = "thumbv6"))]
    /// # bbqtest();
    /// # }
    /// ```
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}

impl<const N: usize, const SUBS: usize> PubSubState<StaticBufferProvider<N>, SUBS> {
    /// Create a new constant static BBQ, using staic memory allocation
    /// ```rust,no_run
    /// use PubSubState::{PubSubState, StaticBufferProvider};
    ///
    /// static mut BUF: PubSubState<StaticBufferProvider<6>> = PubSubState::new_static();
    ///
    /// fn main() {
    ///    let (prod, cons) = BUF.try_split().unwrap();
    /// }
    /// ```
    pub const fn new_static() -> Self {
        Self {
            capacity: N,
            buf: UnsafeCell::new(StaticBufferProvider::new()),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            last: AtomicUsize::new(0),
            reserve: AtomicUsize::new(0),
            read_in_progress: AtomicBool::new(false),
            write_in_progress: AtomicBool::new(false),
            publisher_taken: AtomicBool::new(false),
            publisher_waker: AtomicWaker::new(),
            subscriber_count: AtomicUsize::new(0),
            subscriber_wakers: MultiWakerRegistration::new(),
        }
    }
}
