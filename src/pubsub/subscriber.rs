use crate::pubsub::Error;

use super::{BufferProvider, PubSubChannel};

use super::Result;

use embassy_sync::blocking_mutex::raw::RawMutex;

use core::future::Future;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::ptr::NonNull;
use core::slice::from_raw_parts_mut;
use core::task::{Context, Poll};

/// A subscriber of framed MQTT data
pub struct FrameSubscriber<'a, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    channel: &'a PubSubChannel<M, B, SUBS>,
    next_message_id: u64,
}

impl<'a, M, B, const SUBS: usize> FrameSubscriber<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    pub(super) fn new(channel: &'a PubSubChannel<M, B, SUBS>, next_message_id: u64) -> Self {
        Self {
            channel,
            next_message_id,
        }
    }

    /// Obtain the next available MQTT frame, if any
    pub fn read(&mut self) -> Result<FrameGrantR<'a, M, B, SUBS>> {
        self.read_with_context(None)
    }

    pub fn read_with_context(
        &mut self,
        cx: Option<&mut Context<'_>>,
    ) -> Result<FrameGrantR<'a, M, B, SUBS>> {
        let (start, len, id) = self.channel.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();

            let start_id = inner.next_message_id - inner.messages.len() as u64;

            if self.next_message_id < start_id {
                self.next_message_id = start_id;
                // return Some(WaitResult::Lagged(start_id - message_id));
            }

            let current_message_index = (self.next_message_id - start_id) as usize;

            if current_message_index >= inner.messages.len() {
                if let Some(cx) = cx {
                    inner.subscriber_wakers.register(cx.waker());
                }
                return Err(Error::InsufficientSize);
            }

            // We've checked that the index is valid
            let header = inner
                .messages
                .iter_mut()
                .nth(current_message_index)
                .unwrap();

            if header.read_in_progress {
                if let Some(cx) = cx {
                    inner.subscriber_wakers.register(cx.waker());
                }
                return Err(Error::GrantInProgress);
            }

            // We're reading this item, so decrement the counter & mark it as read in progress
            header.subscriber_count -= 1;
            header.read_in_progress = true;

            self.next_message_id += 1;

            Ok((header.start, header.len, header.message_id))
        })?;

        // Get a mutable slice of the buffer for the granted memory. This is
        // sound, as UnsafeCell, MaybeUninit, and GenericArray are all
        // `#[repr(Transparent)]
        let start_of_buf_ptr = unsafe { (*self.channel.buf.get()).buf().as_mut_ptr().cast::<u8>() };
        let grant_slice = unsafe { from_raw_parts_mut(start_of_buf_ptr.add(start), len as usize) };

        Ok(FrameGrantR {
            channel: self.channel,
            buf: grant_slice.into(),
            message_id: id,
            auto_release: true,
        })
    }

    /// Async version of [Self::read].
    /// Will wait for the buffer to have data to read. When data is available, the grant is returned.
    pub fn read_async(&mut self) -> GrantReadFuture<'a, '_, M, B, SUBS> {
        GrantReadFuture { sub: self }
    }
}

impl<'a, M, B, const SUBS: usize> Drop for FrameSubscriber<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    fn drop(&mut self) {
        self.channel.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();

            inner.subscriber_count -= 1;

            // All messages that haven't been read yet by this subscriber must have their counter decremented
            let start_id = inner.next_message_id - inner.messages.len() as u64;
            if self.next_message_id >= start_id {
                let current_message_index = (self.next_message_id - start_id) as usize;
                inner
                    .messages
                    .iter_mut()
                    .skip(current_message_index)
                    .for_each(|header| header.subscriber_count -= 1);

                let mut wake_publisher = false;
                while let Some(header) = inner.messages.front() {
                    if header.subscriber_count == 0 {
                        inner.messages.pop_front().unwrap();
                        wake_publisher |= true;
                    } else {
                        break;
                    }
                }

                if wake_publisher {
                    inner.publisher_waker.wake();
                }
            }
        })
    }
}

/// Future returned [Subscriber::read_async]
#[must_use = "futures do nothing unless you `.await` or poll them"]
pub struct GrantReadFuture<'a, 'b, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    pub(crate) sub: &'b mut FrameSubscriber<'a, M, B, SUBS>,
}

impl<'a, 'b, M, B, const SUBS: usize> Future for GrantReadFuture<'a, 'b, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    type Output = Result<FrameGrantR<'a, M, B, SUBS>>;

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

/// A read grant for a single frame
///
/// NOTE: If the grant is dropped without explicitly releasing
/// the contents, then no frame will be released.
pub struct FrameGrantR<'a, M, B, const SUBS: usize>
where
    M: RawMutex,
    B: BufferProvider,
{
    pub(crate) channel: &'a PubSubChannel<M, B, SUBS>,
    pub(crate) buf: NonNull<[u8]>,
    pub(crate) message_id: u64,
    pub(crate) auto_release: bool,
}

impl<'a, M, B, const SUBS: usize> Deref for FrameGrantR<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.buf()
    }
}

impl<'a, M, B, const SUBS: usize> DerefMut for FrameGrantR<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    fn deref_mut(&mut self) -> &mut [u8] {
        self.buf_mut()
    }
}

impl<'a, M, B, const SUBS: usize> FrameGrantR<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    /// Obtain access to the inner buffer for reading
    pub fn buf(&self) -> &[u8] {
        unsafe { from_raw_parts_mut(self.buf.as_ptr() as *mut u8, self.buf.len()) }
    }

    /// Obtain mutable access to the read grant
    ///
    /// This is useful if you are performing in-place operations
    /// on an incoming packet, such as decryption
    pub fn buf_mut(&mut self) -> &mut [u8] {
        unsafe { from_raw_parts_mut(self.buf.as_ptr() as *mut u8, self.buf.len()) }
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
        core::mem::transmute::<&[u8], &'static [u8]>(&self.buf())
    }

    /// Release a frame to make the space available for future writing
    ///
    /// Note: The full frame is always released
    pub fn release(mut self) {
        self.auto_release(true);
        drop(self);
    }

    /// Set whether the read frame should be automatically released
    pub fn auto_release(&mut self, is_auto: bool) {
        self.auto_release = is_auto;
    }

    fn release_inner(&mut self) {
        self.channel.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();

            let Some(index) = inner
                .messages
                .iter()
                .position(|m| m.message_id == self.message_id)
            else {
                return;
            };

            // We already checked that the index is valid above
            let header = inner.messages.iter_mut().nth(index).unwrap();

            if !header.read_in_progress {
                return;
            }

            trace!(
                "Releasing {:?} with {} subs @ index {}",
                header.message_id,
                header.subscriber_count,
                index
            );

            if index == 0 && header.subscriber_count == 0 {
                let _ = inner.messages.pop_front().unwrap();
                inner.publisher_waker.wake();
            } else {
                header.read_in_progress = false
            };

            inner.subscriber_wakers.wake();
        })
    }
}

impl<'a, M, B, const SUBS: usize> Drop for FrameGrantR<'a, M, B, SUBS>
where
    M: RawMutex,
    B: BufferProvider,
{
    fn drop(&mut self) {
        if self.auto_release {
            self.release_inner();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::pubsub::Error;
    use crate::pubsub::{buffer_provider::StaticBufferProvider, PubSubChannel};
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use futures::join;

    #[test_log::test]
    fn dropped_subscribers_decrease_count() {
        let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 3> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();

        // Create a subscriber and drop it immediately
        let subscriber1 = pubsub.subscriber().unwrap();

        // Create another subscriber
        let mut subscriber2 = pubsub.subscriber().unwrap();

        // Publish a message
        let mut wgr = publisher.grant(64).unwrap();
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = i as u8;
        }
        wgr.commit(64);

        drop(subscriber1);

        // Read the message with the second subscriber
        let rgr = subscriber2.read().unwrap();
        assert_eq!(rgr.len(), 64);
        rgr.release();

        // Ensure the message is released
        assert!(subscriber2.read().is_err());
    }

    #[test_log::test]
    fn concurrency_test() {
        block_on(async {
            let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 3> =
                PubSubChannel::new(StaticBufferProvider::new());
            let barrier = tokio::sync::Barrier::new(4); // 3 subscribers + 1 publisher

            let subscriber_futures = (0..3).map(|_| {
                let pubsub_ref = &pubsub;
                let barrier_ref = &barrier;
                async move {
                    let mut subscriber = pubsub_ref.subscriber().unwrap();
                    barrier_ref.wait().await; // Wait for all subscribers to be ready

                    for j in 0..10 {
                        let rgr = subscriber.read_async().await.unwrap();
                        assert_eq!(rgr.len(), 64);
                        for (k, by) in rgr.iter().enumerate() {
                            assert_eq!(*by, (j * 10 + k) as u8);
                        }
                        rgr.release();
                    }
                }
            });

            let publisher_future = async {
                barrier.wait().await; // Wait for all subscribers to be ready

                let mut publisher = pubsub.publisher().unwrap();
                for i in 0..10 {
                    let mut wgr = publisher.grant_async(64).await.unwrap();
                    for (j, by) in wgr.iter_mut().enumerate() {
                        *by = (i * 10 + j) as u8;
                    }
                    wgr.commit(64);
                }
            };

            join!(
                publisher_future,
                futures::future::join_all(subscriber_futures)
            );
        });
    }

    #[test_log::test]
    fn not_polling_one_subscriber_does_not_block_others() {
        let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 3> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber1 = pubsub.subscriber().unwrap();
        let mut subscriber2 = pubsub.subscriber().unwrap();

        // Publish a message
        let mut wgr = publisher.grant(64).unwrap();
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = i as u8;
        }
        wgr.commit(64);

        // Read the message with the second subscriber
        let rgr2 = subscriber2.read().unwrap();
        assert_eq!(rgr2.len(), 64);
        rgr2.release();

        // Ensure the first subscriber can still read the message
        let rgr1 = subscriber1.read().unwrap();
        assert_eq!(rgr1.len(), 64);
        rgr1.release();
    }

    #[test_log::test]
    fn multiple_read_grants_different_messages() {
        let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 3> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber1 = pubsub.subscriber().unwrap();
        let mut subscriber2 = pubsub.subscriber().unwrap();

        // Publish two messages
        for i in 0..2 {
            let mut wgr = publisher.grant(64).unwrap();
            for (j, by) in wgr.iter_mut().enumerate() {
                *by = (i * 10 + j) as u8;
            }
            wgr.commit(64);
        }

        // Read the first message with the first subscriber
        let rgr1 = subscriber1.read().unwrap();
        assert_eq!(rgr1.len(), 64);
        for (j, by) in rgr1.iter().enumerate() {
            assert_eq!(*by, j as u8);
        }
        rgr1.release();

        // Read the first message with the second subscriber
        let rgr2 = subscriber2.read().unwrap();
        assert_eq!(rgr2.len(), 64);
        for (j, by) in rgr2.iter().enumerate() {
            assert_eq!(*by, j as u8);
        }

        let rgr3 = subscriber1.read().unwrap();
        assert_eq!(rgr3.len(), 64);
        for (j, by) in rgr3.iter().enumerate() {
            assert_eq!(*by, (10 + j) as u8);
        }

        rgr2.release();
        rgr3.release();
    }

    #[test_log::test]
    fn multiple_read_grants_same_message() {
        let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 3> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber1 = pubsub.subscriber().unwrap();
        let mut subscriber2 = pubsub.subscriber().unwrap();

        // Publish a message
        let mut wgr = publisher.grant(64).unwrap();
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = i as u8;
        }
        wgr.commit(64);

        // Read the message with the first subscriber
        let rgr1 = subscriber1.read().unwrap();
        assert_eq!(rgr1.len(), 64);
        for (j, by) in rgr1.iter().enumerate() {
            assert_eq!(*by, j as u8);
        }

        // Attempt to read the same message with the second subscriber
        assert!(matches!(subscriber2.read(), Err(Error::GrantInProgress)));

        rgr1.release();
    }

    #[test_log::test]
    fn dropping_subscriber_with_active_read_grant() {
        let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 3> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber1 = pubsub.subscriber().unwrap();
        let subscriber2 = pubsub.subscriber().unwrap();

        // Publish a message
        let mut wgr = publisher.grant(64).unwrap();
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = i as u8;
        }
        wgr.commit(64);

        // Read the message with the first subscriber
        let rgr1 = subscriber1.read().unwrap();
        assert_eq!(rgr1.len(), 64);
        for (j, by) in rgr1.iter().enumerate() {
            assert_eq!(*by, j as u8);
        }

        // Drop the second subscriber
        drop(subscriber2);

        // Release the read grant from the first subscriber
        rgr1.release();

        // Ensure the message is released
        assert!(subscriber1.read().is_err());
    }
}
