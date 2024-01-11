use core::cell::RefCell;
use core::cmp::min;
use core::mem::{forget, transmute};
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::slice::from_raw_parts_mut;
use core::task::{Context, Poll};

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex;
use futures::Future;

use super::{state::PubSubState, BufferProvider, Error, Result};

/// `Subscriber` is the primary interface for reading data from a `PubSubState`.
pub struct Subscriber<'a, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    channel: &'a Mutex<M, RefCell<PubSubState<B, SUBS>>>,
}

unsafe impl<'a, M: RawMutex, B: BufferProvider, const SUBS: usize> Send
    for Subscriber<'a, M, B, SUBS>
{
}

impl<'a, M, B, const SUBS: usize> Subscriber<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    pub(super) fn new(channel: &'a Mutex<M, RefCell<PubSubState<B, SUBS>>>) -> Self {
        Self { channel }
    }

    /// Obtains a contiguous slice of committed bytes. This slice may not
    /// contain ALL available bytes, if the writer has wrapped around. The
    /// remaining bytes will be available after all readable bytes are
    /// released
    ///
    /// ```rust
    /// # // PubSubState test shim!
    /// # fn bbqtest() {
    /// use PubSubState::{PubSubState, StaticBufferProvider};
    ///
    /// // Create and split a new buffer of 6 elements
    /// let mut buffer: PubSubState<StaticBufferProvider<6>> = PubSubState::new_static();
    /// let (mut prod, mut cons) = buffer.try_split().unwrap();
    ///
    /// // Successfully obtain and commit a grant of four bytes
    /// let mut grant = prod.grant_max_remaining(4).unwrap();
    /// assert_eq!(grant.buf().len(), 4);
    /// grant.commit(4);
    ///
    /// // Obtain a read grant
    /// let mut grant = cons.read().unwrap();
    /// assert_eq!(grant.buf().len(), 4);
    /// # // PubSubState test shim!
    /// # }
    /// #
    /// # fn main() {
    /// # #[cfg(not(feature = "thumbv6"))]
    /// # bbqtest();
    /// # }
    /// ```
    pub fn read(&mut self) -> Result<GrantR<'a, M, B, SUBS>> {
        self.read_with_context(None)
    }

    pub fn read_with_context(
        &mut self,
        cx: Option<&mut Context<'_>>,
    ) -> Result<GrantR<'a, M, B, SUBS>> {
        self.channel.lock(|channel| {
            let mut inner = channel.borrow_mut();

            if inner.read_in_progress {
                if let Some(cx) = cx {
                    inner.subscriber_wakers.register(cx.waker());
                }
                return Err(Error::GrantInProgress);
            }
            inner.read_in_progress = true;

            let mut read = inner.read;

            // Resolve the inverted case or end of read
            if (read == inner.last) && (inner.write < read) {
                read = 0;
                // This has some room for error, the other thread reads this
                // Impact to Grant:
                //   Grant checks if read < write to see if inverted. If not inverted, but
                //     no space left, Grant will initiate an inversion, but will not trigger it
                // Impact to Commit:
                //   Commit does not check read, but if Grant has started an inversion,
                //   grant could move Last to the prior write position
                // MOVING READ BACKWARDS!
                inner.read = 0;
            }

            let sz = if inner.write < read {
                // Inverted, only believe last
                inner.last
            } else {
                // Not inverted, only believe write
                inner.write
            } - read;

            if sz == 0 {
                inner.read_in_progress = false;
                if let Some(cx) = cx {
                    inner.subscriber_wakers.register(cx.waker());
                }
                return Err(Error::InsufficientSize);
            }

            // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
            // are all `#[repr(Transparent)]
            let start_of_buf_ptr =
                unsafe { (&mut *inner.buf.get()).buf().as_mut_ptr().cast::<u8>() };
            let grant_slice =
                unsafe { from_raw_parts_mut(start_of_buf_ptr.offset(read as isize), sz) };

            Ok(GrantR {
                buf: grant_slice,
                channel: self.channel,
                to_release: 0,
            })
        })
    }

    /// Obtains two disjoint slices, which are each contiguous of committed bytes.
    /// Combined these contain all previously commited data.
    pub fn split_read(&mut self) -> Result<SplitGrantR<'a, M, B, SUBS>> {
        self.split_read_with_context(None)
    }

    pub fn split_read_with_context(
        &mut self,
        cx: Option<&mut Context<'_>>,
    ) -> Result<SplitGrantR<'a, M, B, SUBS>> {
        self.channel.lock(|channel| {
            let mut inner = channel.borrow_mut();

            if inner.read_in_progress {
                if let Some(cx) = cx {
                    inner.subscriber_wakers.register(cx.waker());
                }
                return Err(Error::GrantInProgress);
            }
            inner.read_in_progress = true;

            let mut read = inner.read;

            // Resolve the inverted case or end of read
            if (read == inner.last) && (inner.write < read) {
                read = 0;
                // This has some room for error, the other thread reads this
                // Impact to Grant:
                //   Grant checks if read < write to see if inverted. If not inverted, but
                //     no space left, Grant will initiate an inversion, but will not trigger it
                // Impact to Commit:
                //   Commit does not check read, but if Grant has started an inversion,
                //   grant could move Last to the prior write position
                // MOVING READ BACKWARDS!
                inner.read = 0;
            }

            let (sz1, sz2) = if inner.write < read {
                // Inverted, only believe last
                (inner.last - read, inner.write)
            } else {
                // Not inverted, only believe write
                (inner.write - read, 0)
            };

            if sz1 == 0 {
                inner.read_in_progress = false;
                if let Some(cx) = cx {
                    inner.subscriber_wakers.register(cx.waker());
                }
                return Err(Error::InsufficientSize);
            }

            // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
            // are all `#[repr(Transparent)]
            let start_of_buf_ptr =
                unsafe { (&mut *inner.buf.get()).buf().as_mut_ptr().cast::<u8>() };
            let grant_slice1 =
                unsafe { from_raw_parts_mut(start_of_buf_ptr.offset(read as isize), sz1) };
            let grant_slice2 = unsafe { from_raw_parts_mut(start_of_buf_ptr, sz2) };

            Ok(SplitGrantR {
                buf1: grant_slice1,
                buf2: grant_slice2,
                channel: self.channel,
                to_release: 0,
            })
        })
    }

    /// Async version of [Self::read].
    /// Will wait for the buffer to have data to read. When data is available, the grant is returned.
    pub fn read_async<'b>(&'b mut self) -> GrantReadFuture<'a, 'b, M, B, SUBS> {
        GrantReadFuture { sub: self }
    }

    /// Async version of [Self::split_read].
    /// Will wait just like [Self::read_async], but returns the split grant to obtain all the available data.
    pub fn split_read_async<'b>(&'b mut self) -> GrantSplitReadFuture<'a, 'b, M, B, SUBS> {
        GrantSplitReadFuture { sub: self }
    }
}

impl<'a, M, B, const SUBS: usize> Drop for Subscriber<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.channel.lock(|channel| {
            let mut inner = channel.borrow_mut();
            inner.subscriber_count -= 1;
        });
    }
}

/// Future returned [Subscriber::read_async]
pub struct GrantReadFuture<'a, 'b, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    pub(crate) sub: &'b mut Subscriber<'a, M, B, SUBS>,
}

impl<'a, 'b, M, B, const SUBS: usize> Future for GrantReadFuture<'a, 'b, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    type Output = Result<GrantR<'a, M, B, SUBS>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.sub.read_with_context(Some(cx)) {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(_) => Poll::Pending,
        }
    }
}

impl<'a, 'b, M: RawMutex, B: BufferProvider, const SUBS: usize> Unpin
    for GrantReadFuture<'a, 'b, M, B, SUBS>
{
}

/// Future returned [Subscriber::split_read_async]
pub struct GrantSplitReadFuture<'a, 'b, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    pub(crate) sub: &'b mut Subscriber<'a, M, B, SUBS>,
}

impl<'a, 'b, M, B, const SUBS: usize> Future for GrantSplitReadFuture<'a, 'b, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    type Output = Result<SplitGrantR<'a, M, B, SUBS>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.sub.split_read_with_context(Some(cx)) {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(_) => Poll::Pending,
        }
    }
}

impl<'a, 'b, M: RawMutex, B: BufferProvider, const SUBS: usize> Unpin
    for GrantSplitReadFuture<'a, 'b, M, B, SUBS>
{
}

/// A structure representing a contiguous region of memory that
/// may be read from, and potentially "released" (or cleared)
/// from the queue
///
/// NOTE: If the grant is dropped without explicitly releasing
/// the contents, or by setting the number of bytes to automatically
/// be released with `to_release()`, then no bytes will be released
/// as read.
///
///
/// If the `thumbv6` feature is selected, dropping the grant
/// without releasing it takes a short critical section,
pub struct GrantR<'a, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    pub(crate) buf: &'a mut [u8],
    pub(crate) channel: &'a Mutex<M, RefCell<PubSubState<B, SUBS>>>,
    pub(crate) to_release: usize,
}

/// A structure representing up to two contiguous regions of memory that
/// may be read from, and potentially "released" (or cleared)
/// from the queue
pub struct SplitGrantR<'a, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    pub(crate) buf1: &'a mut [u8],
    pub(crate) buf2: &'a mut [u8],
    channel: &'a Mutex<M, RefCell<PubSubState<B, SUBS>>>,
    pub(crate) to_release: usize,
}

unsafe impl<'a, M: RawMutex, B: BufferProvider, const SUBS: usize> Send for GrantR<'a, M, B, SUBS> {}

unsafe impl<'a, M: RawMutex, B: BufferProvider, const SUBS: usize> Send
    for SplitGrantR<'a, M, B, SUBS>
{
}

impl<'a, M, B, const SUBS: usize> GrantR<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    /// Release a sequence of bytes from the buffer, allowing the space
    /// to be used by later writes. This consumes the grant.
    ///
    /// If `used` is larger than the given grant, the full grant will
    /// be released.
    ///
    /// NOTE:  If the `thumbv6` feature is selected, this function takes a short critical
    /// section while releasing.
    pub fn release(mut self, used: usize) {
        // Saturate the grant release
        let used = min(self.buf.len(), used);

        self.release_inner(used);
        forget(self);
    }

    pub(crate) fn shrink(&mut self, len: usize) {
        let mut new_buf: &mut [u8] = &mut [];
        core::mem::swap(&mut self.buf, &mut new_buf);
        let (new, _) = new_buf.split_at_mut(len);
        self.buf = new;
    }

    /// Obtain access to the inner buffer for reading
    ///
    /// ```
    /// # // PubSubState test shim!
    /// # fn bbqtest() {
    /// use PubSubState::{PubSubState, StaticBufferProvider};
    ///
    /// // Create and split a new buffer of 6 elements
    /// let mut buffer: PubSubState<StaticBufferProvider<6>> = PubSubState::new_static();
    /// let (mut prod, mut cons) = buffer.try_split().unwrap();
    ///
    /// // Successfully obtain and commit a grant of four bytes
    /// let mut grant = prod.grant_max_remaining(4).unwrap();
    /// grant.buf().copy_from_slice(&[1, 2, 3, 4]);
    /// grant.commit(4);
    ///
    /// // Obtain a read grant, and copy to a buffer
    /// let mut grant = cons.read().unwrap();
    /// let mut buf = [0u8; 4];
    /// buf.copy_from_slice(grant.buf());
    /// assert_eq!(&buf, &[1, 2, 3, 4]);
    /// # // PubSubState test shim!
    /// # }
    /// #
    /// # fn main() {
    /// # #[cfg(not(feature = "thumbv6"))]
    /// # bbqtest();
    /// # }
    /// ```
    pub fn buf(&self) -> &[u8] {
        self.buf
    }

    /// Obtain mutable access to the read grant
    ///
    /// This is useful if you are performing in-place operations
    /// on an incoming packet, such as decryption
    pub fn buf_mut(&mut self) -> &mut [u8] {
        self.buf
    }

    /// Sometimes, it's not possible for the lifetimes to check out. For example,
    /// if you need to hand this buffer to a function that expects to receive a
    /// `&'static [u8]`, it is not possible for the inner reference to outlive the
    /// grant itself.
    ///
    /// You MUST guarantee that in no cases, the reference that is returned here outlives
    /// the grant itself. Once the grant has been released, referencing the data contained
    /// WILL cause undefined behavior.
    ///
    /// Additionally, you must ensure that a separate reference to this data is not created
    /// to this data, e.g. using `Deref` or the `buf()` method of this grant.
    pub unsafe fn as_static_buf(&self) -> &'static [u8] {
        transmute::<&[u8], &'static [u8]>(self.buf)
    }

    #[inline(always)]
    pub(crate) fn release_inner(&mut self, used: usize) {
        self.channel.lock(|channel| {
            let mut inner = channel.borrow_mut();

            // If there is no grant in progress, return early. This
            // generally means we are dropping the grant within a
            // wrapper structure
            if !inner.read_in_progress {
                return;
            }

            // This should always be checked by the public interfaces
            debug_assert!(used <= self.buf.len());

            // This should be fine, purely incrementing
            inner.read += used;

            inner.read_in_progress = false;
            inner.publisher_waker.wake();
        })
    }

    /// Configures the amount of bytes to be released on drop.
    pub fn to_release(&mut self, amt: usize) {
        self.to_release = self.buf.len().min(amt);
    }
}

impl<'a, M, B, const SUBS: usize> SplitGrantR<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    /// Release a sequence of bytes from the buffer, allowing the space
    /// to be used by later writes. This consumes the grant.
    ///
    /// If `used` is larger than the given grant, the full grant will
    /// be released.
    ///
    /// NOTE:  If the `thumbv6` feature is selected, this function takes a short critical
    /// section while releasing.
    pub fn release(mut self, used: usize) {
        // Saturate the grant release
        let used = min(self.combined_len(), used);

        self.release_inner(used);
        forget(self);
    }

    /// Obtain access to both inner buffers for reading
    ///
    /// ```
    /// # // PubSubState test shim!
    /// # fn bbqtest() {
    /// use PubSubState::{PubSubState, StaticBufferProvider};
    ///
    /// // Create and split a new buffer of 6 elements
    /// let mut buffer: PubSubState<StaticBufferProvider<6>> = PubSubState::new_static();
    /// let (mut prod, mut cons) = buffer.try_split().unwrap();
    ///
    /// // Successfully obtain and commit a grant of four bytes
    /// let mut grant = prod.grant_max_remaining(4).unwrap();
    /// grant.buf().copy_from_slice(&[1, 2, 3, 4]);
    /// grant.commit(4);
    ///
    /// // Obtain a read grant, and copy to a buffer
    /// let mut grant = cons.read().unwrap();
    /// let mut buf = [0u8; 4];
    /// buf.copy_from_slice(grant.buf());
    /// assert_eq!(&buf, &[1, 2, 3, 4]);
    /// # // PubSubState test shim!
    /// # }
    /// #
    /// # fn main() {
    /// # #[cfg(not(feature = "thumbv6"))]
    /// # bbqtest();
    /// # }
    /// ```
    pub fn bufs(&self) -> (&[u8], &[u8]) {
        (self.buf1, self.buf2)
    }

    /// Obtain mutable access to both parts of the read grant
    ///
    /// This is useful if you are performing in-place operations
    /// on an incoming packet, such as decryption
    pub fn bufs_mut(&mut self) -> (&mut [u8], &mut [u8]) {
        (self.buf1, self.buf2)
    }

    #[inline(always)]
    pub(crate) fn release_inner(&mut self, used: usize) {
        self.channel.lock(|channel| {
            let mut inner = channel.borrow_mut();

            // If there is no grant in progress, return early. This
            // generally means we are dropping the grant within a
            // wrapper structure
            if !inner.read_in_progress {
                return;
            }

            // This should always be checked by the public interfaces
            debug_assert!(used <= self.combined_len());

            if used <= self.buf1.len() {
                // This should be fine, purely incrementing
                inner.read += used;
            } else {
                // Also release parts of the second buffer
                inner.read = used - self.buf1.len();
            }

            inner.read_in_progress = false;
        })
    }

    /// Configures the amount of bytes to be released on drop.
    pub fn to_release(&mut self, amt: usize) {
        self.to_release = self.combined_len().min(amt);
    }

    /// The combined length of both buffers
    pub fn combined_len(&self) -> usize {
        self.buf1.len() + self.buf2.len()
    }
}

impl<'a, M, B, const SUBS: usize> Drop for GrantR<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.release_inner(self.to_release)
    }
}

impl<'a, M, B, const SUBS: usize> Drop for SplitGrantR<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.release_inner(self.to_release)
    }
}

impl<'a, M, B, const SUBS: usize> Deref for GrantR<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.buf
    }
}

impl<'a, M, B, const SUBS: usize> DerefMut for GrantR<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    fn deref_mut(&mut self) -> &mut [u8] {
        self.buf
    }
}
