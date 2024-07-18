use core::cmp::min;
use core::mem::{forget, transmute};
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::slice::from_raw_parts_mut;
use core::sync::atomic::Ordering::{AcqRel, Acquire, Release};
use core::task::{Context, Poll};

use futures::Future;

use crate::pubsub::atomic;

use super::{BufferProvider, Error, PubSubChannel, Result};

/// `Subscriber` is the primary interface for reading data from a `PubSubState`.
pub struct Subscriber<'a, B, const SUBS: usize>
where
    B: BufferProvider,
{
    channel: &'a PubSubChannel<B, SUBS>,
}

unsafe impl<'a, B: BufferProvider, const SUBS: usize> Send for Subscriber<'a, B, SUBS> {}

impl<'a, B, const SUBS: usize> Subscriber<'a, B, SUBS>
where
    B: BufferProvider,
{
    pub(super) fn new(channel: &'a PubSubChannel<B, SUBS>) -> Self {
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
    pub fn read(&mut self) -> Result<GrantR<'a, B, SUBS>> {
        self.read_with_context(None)
    }

    pub fn read_with_context(
        &mut self,
        cx: Option<&mut Context<'_>>,
    ) -> Result<GrantR<'a, B, SUBS>> {
        let inner = self.channel;

        if atomic::swap(&inner.read_in_progress, true, AcqRel) {
            if let Some(cx) = cx {
                inner.subscriber_wakers.register(cx.waker());
            }
            return Err(Error::GrantInProgress);
        }

        let write = inner.write.load(Acquire);
        let last = inner.last.load(Acquire);
        let mut read = inner.read.load(Acquire);

        // Resolve the inverted case or end of read
        if (read == last) && (write < read) {
            read = 0;
            // This has some room for error, the other thread reads this
            // Impact to Grant:
            //   Grant checks if read < write to see if inverted. If not inverted, but
            //     no space left, Grant will initiate an inversion, but will not trigger it
            // Impact to Commit:
            //   Commit does not check read, but if Grant has started an inversion,
            //   grant could move Last to the prior write position
            // MOVING READ BACKWARDS!
            inner.read.store(0, Release);
        }

        let sz = if write < read {
            // Inverted, only believe last
            last
        } else {
            // Not inverted, only believe write
            write
        } - read;

        if sz == 0 {
            inner.read_in_progress.store(false, Release);
            if let Some(cx) = cx {
                inner.subscriber_wakers.register(cx.waker());
            }
            return Err(Error::InsufficientSize);
        }

        // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
        // are all `#[repr(Transparent)]
        let start_of_buf_ptr = unsafe { (*inner.buf.get()).buf().as_mut_ptr().cast::<u8>() };
        let grant_slice = unsafe { from_raw_parts_mut(start_of_buf_ptr.add(read), sz) };

        Ok(GrantR {
            buf: grant_slice,
            channel: self.channel,
            to_release: 0,
        })
    }

    /// Obtains two disjoint slices, which are each contiguous of committed bytes.
    /// Combined these contain all previously committed data.
    pub fn split_read(&mut self) -> Result<SplitGrantR<'a, B, SUBS>> {
        self.split_read_with_context(None)
    }

    pub fn split_read_with_context(
        &mut self,
        cx: Option<&mut Context<'_>>,
    ) -> Result<SplitGrantR<'a, B, SUBS>> {
        let inner = self.channel;

        if atomic::swap(&inner.read_in_progress, true, AcqRel) {
            if let Some(cx) = cx {
                inner.subscriber_wakers.register(cx.waker());
            }
            return Err(Error::GrantInProgress);
        }

        let write = inner.write.load(Acquire);
        let last = inner.last.load(Acquire);
        let mut read = inner.read.load(Acquire);

        // Resolve the inverted case or end of read
        if (read == last) && (write < read) {
            read = 0;
            // This has some room for error, the other thread reads this
            // Impact to Grant:
            //   Grant checks if read < write to see if inverted. If not inverted, but
            //     no space left, Grant will initiate an inversion, but will not trigger it
            // Impact to Commit:
            //   Commit does not check read, but if Grant has started an inversion,
            //   grant could move Last to the prior write position
            // MOVING READ BACKWARDS!
            inner.read.store(0, Release);
        }

        let (sz1, sz2) = if write < read {
            // Inverted, only believe last
            (last - read, write)
        } else {
            // Not inverted, only believe write
            (write - read, 0)
        };

        if sz1 == 0 {
            inner.read_in_progress.store(false, Release);
            if let Some(cx) = cx {
                inner.subscriber_wakers.register(cx.waker());
            }
            return Err(Error::InsufficientSize);
        }

        // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
        // are all `#[repr(Transparent)]
        let start_of_buf_ptr = unsafe { (*inner.buf.get()).buf().as_mut_ptr().cast::<u8>() };
        let grant_slice1 = unsafe { from_raw_parts_mut(start_of_buf_ptr.add(read), sz1) };
        let grant_slice2 = unsafe { from_raw_parts_mut(start_of_buf_ptr, sz2) };

        Ok(SplitGrantR {
            buf1: grant_slice1,
            buf2: grant_slice2,
            channel: self.channel,
            to_release: 0,
        })
    }

    /// Async version of [Self::read].
    /// Will wait for the buffer to have data to read. When data is available, the grant is returned.
    pub fn read_async<'b>(&'b mut self) -> GrantReadFuture<'a, 'b, B, SUBS> {
        GrantReadFuture { sub: self }
    }

    /// Async version of [Self::split_read].
    /// Will wait just like [Self::read_async], but returns the split grant to obtain all the available data.
    pub fn split_read_async<'b>(&'b mut self) -> GrantSplitReadFuture<'a, 'b, B, SUBS> {
        GrantSplitReadFuture { sub: self }
    }
}

impl<'a, B, const SUBS: usize> Drop for Subscriber<'a, B, SUBS>
where
    B: BufferProvider,
{
    fn drop(&mut self) {
        let inner = self.channel;
        atomic::fetch_sub(&inner.subscriber_count, 1, AcqRel);
    }
}

/// Future returned [Subscriber::read_async]
pub struct GrantReadFuture<'a, 'b, B, const SUBS: usize>
where
    B: BufferProvider,
{
    pub(crate) sub: &'b mut Subscriber<'a, B, SUBS>,
}

impl<'a, 'b, B, const SUBS: usize> Future for GrantReadFuture<'a, 'b, B, SUBS>
where
    B: BufferProvider,
{
    type Output = Result<GrantR<'a, B, SUBS>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.sub.read_with_context(Some(cx)) {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(_) => Poll::Pending,
        }
    }
}

impl<'a, 'b, B: BufferProvider, const SUBS: usize> Unpin for GrantReadFuture<'a, 'b, B, SUBS> {}

/// Future returned [Subscriber::split_read_async]
pub struct GrantSplitReadFuture<'a, 'b, B, const SUBS: usize>
where
    B: BufferProvider,
{
    pub(crate) sub: &'b mut Subscriber<'a, B, SUBS>,
}

impl<'a, 'b, B, const SUBS: usize> Future for GrantSplitReadFuture<'a, 'b, B, SUBS>
where
    B: BufferProvider,
{
    type Output = Result<SplitGrantR<'a, B, SUBS>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.sub.split_read_with_context(Some(cx)) {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(_) => Poll::Pending,
        }
    }
}

impl<'a, 'b, B: BufferProvider, const SUBS: usize> Unpin for GrantSplitReadFuture<'a, 'b, B, SUBS> {}

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
pub struct GrantR<'a, B, const SUBS: usize>
where
    B: BufferProvider,
{
    pub(crate) buf: &'a mut [u8],
    pub(crate) channel: &'a PubSubChannel<B, SUBS>,
    pub(crate) to_release: usize,
}

/// A structure representing up to two contiguous regions of memory that
/// may be read from, and potentially "released" (or cleared)
/// from the queue
pub struct SplitGrantR<'a, B, const SUBS: usize>
where
    B: BufferProvider,
{
    pub(crate) buf1: &'a mut [u8],
    pub(crate) buf2: &'a mut [u8],
    channel: &'a PubSubChannel<B, SUBS>,
    pub(crate) to_release: usize,
}

unsafe impl<'a, B: BufferProvider, const SUBS: usize> Send for GrantR<'a, B, SUBS> {}

unsafe impl<'a, B: BufferProvider, const SUBS: usize> Send for SplitGrantR<'a, B, SUBS> {}

impl<'a, B, const SUBS: usize> GrantR<'a, B, SUBS>
where
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
        let inner = self.channel;

        // If there is no grant in progress, return early. This
        // generally means we are dropping the grant within a
        // wrapper structure
        if !inner.read_in_progress.load(Acquire) {
            return;
        }

        // This should always be checked by the public interfaces
        debug_assert!(used <= self.buf.len());

        // This should be fine, purely incrementing
        let _ = atomic::fetch_add(&inner.read, used, Release);

        inner.read_in_progress.store(false, Release);

        inner.publisher_waker.wake();
    }

    /// Configures the amount of bytes to be released on drop.
    pub fn to_release(&mut self, amt: usize) {
        self.to_release = self.buf.len().min(amt);
    }
}

impl<'a, B, const SUBS: usize> SplitGrantR<'a, B, SUBS>
where
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
        let inner = self.channel;

        // If there is no grant in progress, return early. This
        // generally means we are dropping the grant within a
        // wrapper structure
        if !inner.read_in_progress.load(Acquire) {
            return;
        }

        // This should always be checked by the public interfaces
        debug_assert!(used <= self.combined_len());

        if used <= self.buf1.len() {
            // This should be fine, purely incrementing
            let _ = atomic::fetch_add(&inner.read, used, Release);
        } else {
            // Also release parts of the second buffer
            inner.read.store(used - self.buf1.len(), Release);
        }

        inner.read_in_progress.store(false, Release);
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

impl<'a, B, const SUBS: usize> Drop for GrantR<'a, B, SUBS>
where
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.release_inner(self.to_release)
    }
}

impl<'a, B, const SUBS: usize> Drop for SplitGrantR<'a, B, SUBS>
where
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.release_inner(self.to_release)
    }
}

impl<'a, B, const SUBS: usize> Deref for GrantR<'a, B, SUBS>
where
    B: BufferProvider,
{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.buf
    }
}

impl<'a, B, const SUBS: usize> DerefMut for GrantR<'a, B, SUBS>
where
    B: BufferProvider,
{
    fn deref_mut(&mut self) -> &mut [u8] {
        self.buf
    }
}
