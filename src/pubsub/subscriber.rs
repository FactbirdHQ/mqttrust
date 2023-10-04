//! Implementation of anything directly subscriber related

use core::future::Future;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll};

use super::{PubSubBehavior, PubSubChannel};
use embassy_sync::blocking_mutex::raw::RawMutex;

/// A subscriber to a channel
pub struct Sub<'a, PSB: PubSubBehavior + ?Sized> {
    /// The message id of the next message we are yet to receive
    next_message_id: u64,
    /// The channel we are a subscriber to
    channel: &'a PSB,
}

impl<'a, PSB: PubSubBehavior + ?Sized> Sub<'a, PSB> {
    pub(super) fn new(next_message_id: u64, channel: &'a PSB) -> Self {
        Self {
            next_message_id,
            channel,
        }
    }

    /// Wait for a published message
    pub fn next_message<'s>(&'s mut self) -> SubscriberWaitFuture<'s, 'a, PSB> {
        SubscriberWaitFuture { subscriber: self }
    }

    /// Wait for a published message (ignoring lag results)
    pub async fn next_message_pure(&mut self) -> T {
        loop {
            match self.next_message().await {
                WaitResult::Lagged(_) => continue,
                WaitResult::Message(message) => break message,
            }
        }
    }

    /// Try to see if there's a published message we haven't received yet.
    ///
    /// This function does not peek. The message is received if there is one.
    pub fn try_next_message(&mut self) -> Option<WaitResult> {
        match self.channel.get_message_with_context(&mut self.next_message_id, None) {
            Poll::Ready(result) => Some(result),
            Poll::Pending => None,
        }
    }

    /// Try to see if there's a published message we haven't received yet (ignoring lag results).
    ///
    /// This function does not peek. The message is received if there is one.
    pub fn try_next_message_pure(&mut self) -> Option<T> {
        loop {
            match self.try_next_message() {
                Some(WaitResult::Lagged(_)) => continue,
                Some(WaitResult::Message(message)) => break Some(message),
                None => break None,
            }
        }
    }

    /// The amount of messages this subscriber hasn't received yet
    pub fn available(&self) -> u64 {
        self.channel.available(self.next_message_id)
    }
}

impl<'a, PSB: PubSubBehavior + ?Sized> Drop for Sub<'a, PSB> {
    fn drop(&mut self) {
        self.channel.unregister_subscriber(self.next_message_id)
    }
}

impl<'a, PSB: PubSubBehavior + ?Sized> Unpin for Sub<'a, PSB> {}

/// Warning: The stream implementation ignores lag results and returns all messages.
/// This might miss some messages without you knowing it.
impl<'a, PSB: PubSubBehavior + ?Sized> futures::Stream for Sub<'a, PSB> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self
            .channel
            .get_message_with_context(&mut self.next_message_id, Some(cx))
        {
            Poll::Ready(WaitResult::Message(message)) => Poll::Ready(Some(message)),
            Poll::Ready(WaitResult::Lagged(_)) => {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// A subscriber that holds a dynamic reference to the channel
pub struct DynSubscriber<'a>(pub(super) Sub<'a, dyn PubSubBehavior + 'a>);

impl<'a> Deref for DynSubscriber<'a> {
    type Target = Sub<'a, dyn PubSubBehavior + 'a>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> DerefMut for DynSubscriber<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// A subscriber that holds a generic reference to the channel
pub struct Subscriber<'a, M: RawMutex, const CAP: usize, const SUBS: usize>(
    pub(super) Sub<'a, PubSubChannel<M, CAP, SUBS>>,
);

impl<'a, M: RawMutex, const CAP: usize, const SUBS: usize> Deref
    for Subscriber<'a, M, CAP, SUBS>
{
    type Target = Sub<'a, PubSubChannel<M, CAP, SUBS>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, M: RawMutex, const CAP: usize, const SUBS: usize> DerefMut
    for Subscriber<'a, M, CAP, SUBS>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
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
#[derive(Debug, PartialEq)]
pub struct GrantR<'a> {
    pub(crate) buf: &'a mut [u8],
    bbq: NonNull<BBQueue<B>>,
    pub(crate) to_release: usize,
}

/// A structure representing up to two contiguous regions of memory that
/// may be read from, and potentially "released" (or cleared)
/// from the queue
#[derive(Debug, PartialEq)]
pub struct SplitGrantR<'a> {
    pub(crate) buf1: &'a mut [u8],
    pub(crate) buf2: &'a mut [u8],
    bbq: NonNull<BBQueue<B>>,
    pub(crate) to_release: usize,
}

impl<'a> GrantR<'a> {
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
        core::mem::forget(self);
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
    /// # // bbqueue test shim!
    /// # fn bbqtest() {
    /// use bbqueue::{BBQueue, StaticBufferProvider};
    ///
    /// // Create and split a new buffer of 6 elements
    /// let buffer: BBQueue<StaticBufferProvider<6>> = BBQueue::new_static();
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
    /// # // bbqueue test shim!
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
        core::mem::transmute::<&[u8], &'static [u8]>(self.buf)
    }

    #[inline(always)]
    pub(crate) fn release_inner(&mut self, used: usize) {
        let inner = unsafe { &self.bbq.as_ref() };

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
        unsafe { self.bbq.as_mut().write_waker.wake() };
    }

    /// Configures the amount of bytes to be released on drop.
    pub fn to_release(&mut self, amt: usize) {
        self.to_release = self.buf.len().min(amt);
    }
}

impl<'a> SplitGrantR<'a> {
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
        core::mem::forget(self);
    }

    /// Obtain access to both inner buffers for reading
    ///
    /// ```
    /// # // bbqueue test shim!
    /// # fn bbqtest() {
    /// use bbqueue::{BBQueue, StaticBufferProvider};
    ///
    /// // Create and split a new buffer of 6 elements
    /// let buffer: BBQueue<StaticBufferProvider<6>> = BBQueue::new_static();
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
    /// # // bbqueue test shim!
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
        let inner = unsafe { &self.bbq.as_ref() };

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

impl<'a> Drop for GrantR<'a> {
    fn drop(&mut self) {
        self.release_inner(self.to_release)
    }
}

impl<'a> Drop for SplitGrantR<'a> {
    fn drop(&mut self) {
        self.release_inner(self.to_release)
    }
}

impl<'a> Deref for GrantR<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.buf
    }
}

impl<'a> DerefMut for GrantR<'a> {
    fn deref_mut(&mut self) -> &mut [u8] {
        self.buf
    }
}

/// Future returned [Consumer::read_async]
pub struct GrantReadFuture<'a, 'b> {
    cons: &'b mut Consumer<'a>,
}

impl<'a, 'b> Future for GrantReadFuture<'a, 'b> {
    type Output = Result<GrantR<'a>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.cons.read() {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(e) => match e {
                Error::InsufficientSize | Error::GrantInProgress => {
                    unsafe { self.cons.bbq.as_mut().read_waker.set(cx.waker()) };
                    Poll::Pending
                }
                _ => Poll::Ready(Err(e)),
            },
        }
    }
}

/// Future returned [Consumer::split_read_async]
pub struct GrantSplitReadFuture<'a, 'b> {
    cons: &'b mut Consumer<'a>,
}

impl<'a, 'b> Future for GrantSplitReadFuture<'a, 'b> {
    type Output = Result<SplitGrantR<'a>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.cons.split_read() {
            Ok(grant) => Poll::Ready(Ok(grant)),
            Err(e) => match e {
                Error::InsufficientSize | Error::GrantInProgress => {
                    unsafe { self.cons.bbq.as_mut().read_waker.set(cx.waker()) };
                    Poll::Pending
                }
                _ => Poll::Ready(Err(e)),
            },
        }
    }
}
