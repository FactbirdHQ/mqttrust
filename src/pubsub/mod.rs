//! Publish/Subscribe queue directly targeted towards internal usage in an MQTT
//! client implementation.
//!
//! This implementation is highly inspired by both `embassy_sync::PubSub`, and
//! `bbqueue`. Full credit to those projects for the complex internals.

mod publisher;
mod subscriber;

use core::cell::{RefCell, UnsafeCell};

use embassy_sync::{
    blocking_mutex::{raw::RawMutex, Mutex},
    waitqueue::{MultiWakerRegistration, WakerRegistration},
};
pub use publisher::*;
pub use subscriber::*;

mod buffer_provider;
use crate::crate_config::MAX_PUBSUB_MESSAGES;
pub use buffer_provider::*;

/// Result type used by the pubsub
pub type Result<T> = core::result::Result<T, Error>;

/// Error type used by the `PubSub` interfaces
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    /// The buffer does not contain sufficient size for the requested action
    InsufficientSize,

    /// Unable to produce another grant, a grant of this type is already in
    /// progress
    GrantInProgress,

    /// The publisher has already been taken and cannot be created again.
    PublisherAlreadyTaken,

    /// The maximum number of subscribers has been reached.
    MaximumSubscribersReached,
}

/// Header information for each message in the pubsub queue.
#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Header {
    start: usize,
    len: u16,
    message_id: u64,
    subscriber_count: u8,
    read_in_progress: bool,
    pub(super) pid: u16,
}

/// Internal state of the pubsub channel.
struct State<const SUBS: usize> {
    capacity: usize,
    messages: heapless::Deque<Header, { MAX_PUBSUB_MESSAGES }>,
    write_in_progress: bool,
    next_message_id: u64,
    subscriber_count: u8,
    publisher_taken: bool,
    subscriber_wakers: MultiWakerRegistration<SUBS>,
    publisher_waker: WakerRegistration,
}

impl<const SUBS: usize> State<SUBS> {
    /// Clears all messages in the channel.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Returns the number of messages currently in the channel.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Returns whether the channel is empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Returns whether the channel is full.
    pub fn is_full(&self) -> bool {
        self.messages.is_full() || self.free_space() == 0
    }

    /// Returns the amount of free space available in the channel in bytes.
    pub fn free_space(&self) -> usize {
        match self.messages.back() {
            Some(back) => {
                // We have a back, so we know we also have a front, though they may be the same item
                let front = self.messages.front().unwrap();

                let back_end = back.start + back.len as usize;
                let inverted = back_end < front.start;

                if inverted {
                    front.start - back_end
                } else {
                    // This will also always catch the case where `front ==
                    // back` because we only have one element
                    front.start.max(self.capacity - back_end)
                }
            }
            // Empty channel means we have the full capacity available
            None => self.capacity,
        }
    }
}

/// Publish/Subscribe channel for internal MQTT client usage.
///
/// This struct manages the pubsub queue and provides methods for creating publishers and subscribers.
pub struct PubSubChannel<M: RawMutex, B: BufferProvider, const SUBS: usize> {
    buf: UnsafeCell<B>,
    inner: Mutex<M, RefCell<State<SUBS>>>,
}

impl<M: RawMutex, B: BufferProvider, const SUBS: usize> PubSubChannel<M, B, SUBS> {
    /// Creates a new `PubSubChannel`.
    ///
    /// # Parameters
    ///
    /// - `buf`: The buffer provider for the channel.
    ///
    /// # Returns
    ///
    /// A new instance of `PubSubChannel`.
    pub fn new(mut buf: B) -> Self {
        Self {
            inner: Mutex::new(RefCell::new(State {
                capacity: buf.buf().len(),
                messages: heapless::Deque::new(),
                write_in_progress: false,
                publisher_taken: false,
                subscriber_count: 0,
                next_message_id: 0,
                publisher_waker: WakerRegistration::new(),
                subscriber_wakers: MultiWakerRegistration::new(),
            })),
            buf: UnsafeCell::new(buf),
        }
    }

    /// Creates a new `FrameSubscriber`. It will only receive messages that are published after its creation.
    ///
    /// If there are no subscriber slots left, an error will be returned.
    ///
    /// # Returns
    ///
    /// `Ok(FrameSubscriber)` if a subscriber was successfully created, otherwise an `Error`.
    pub fn subscriber(&self) -> Result<FrameSubscriber<'_, M, B, SUBS>> {
        self.inner.lock(|i| {
            let mut inner = i.borrow_mut();
            if inner.subscriber_count >= SUBS as u8 {
                return Err(Error::MaximumSubscribersReached);
            }
            inner.subscriber_count += 1;
            Ok(FrameSubscriber::new(self, inner.next_message_id))
        })
    }

    /// Creates a new `FramePublisher`.
    ///
    /// If the publisher has already been taken, an error will be returned.
    ///
    /// # Returns
    ///
    /// `Ok(FramePublisher)` if a publisher was successfully created, otherwise an `Error`.
    pub fn publisher(&self) -> Result<FramePublisher<'_, M, B, SUBS>> {
        self.inner.lock(|i| {
            let mut inner = i.borrow_mut();
            if inner.publisher_taken {
                return Err(Error::PublisherAlreadyTaken);
            }
            inner.publisher_taken = true;
            Ok(())
        })?;

        unsafe {
            // Explicitly zero the data to avoid undefined behavior.
            // This is required, because we hand out references to the buffers,
            // which mean that creating them as references is technically UB for now
            let mu_ptr = (*self.buf.get()).buf();
            core::ptr::write_bytes(mu_ptr.as_mut_ptr(), 0, mu_ptr.len());
        }

        Ok(FramePublisher::new(self))
    }

    /// Returns the number of active subscribers.
    pub fn subscribers(&self) -> u8 {
        self.inner.lock(|inner| inner.borrow().subscriber_count)
    }

    /// Clears all messages in the channel.
    pub fn clear(&self) {
        self.inner.lock(|inner| inner.borrow_mut().clear());
    }

    /// Returns the number of messages currently in the channel.
    pub fn len(&self) -> usize {
        self.inner.lock(|inner| inner.borrow().len())
    }

    /// Returns whether the channel is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.lock(|inner| inner.borrow().is_empty())
    }

    /// Returns whether the channel is full.
    pub fn is_full(&self) -> bool {
        self.inner.lock(|inner| inner.borrow().is_full())
    }

    /// Returns the amount of free space available in the channel in bytes.
    pub fn free_space(&self) -> usize {
        self.inner.lock(|inner| inner.borrow().free_space())
    }
}

impl<'a, M, const SUBS: usize> PubSubChannel<M, SliceBufferProvider<'a>, SUBS>
where
    M: RawMutex,
{
    /// Creates a new `PubSubChannel` from a slice.
    ///
    /// # Parameters
    ///
    /// - `buf`: The buffer slice.
    ///
    /// # Returns
    ///
    /// A new instance of `PubSubChannel`.
    pub fn new_from_slice(buf: &'a mut [u8]) -> Self {
        Self::new(SliceBufferProvider::new(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::{PubSubChannel, StaticBufferProvider};
    use embassy_futures::block_on;
    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

    #[test_log::test]
    fn frame_wrong_size() {
        let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();

        // Create largeish grants
        let mut wgr = publisher.grant(127).unwrap();
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = i as u8;
        }

        wgr.commit(256);

        let rgr = subscriber.read().unwrap();
        assert_eq!(rgr.len(), 127);
        for (i, by) in rgr.iter().enumerate() {
            assert_eq!((i as u8), *by);
        }
        rgr.release();
    }

    #[test_log::test]
    fn full_size() {
        let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();
        let mut ctr = 0;

        for _ in 0..100 {
            // Create largeish grants
            if let Ok(mut wgr) = publisher.grant(127) {
                ctr += 1;
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = i as u8;
                }
                wgr.commit(127);

                let rgr = subscriber.read().unwrap();
                assert_eq!(rgr.len(), 127);
                for (i, by) in rgr.iter().enumerate() {
                    assert_eq!((i as u8), *by);
                }
                rgr.release();
            } else {
                // Create smallish grants
                let mut wgr = publisher.grant(1).unwrap();
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = i as u8;
                }
                wgr.commit(1);

                let rgr = subscriber.read().unwrap();
                assert_eq!(rgr.len(), 1);
                for (i, by) in rgr.iter().enumerate() {
                    assert_eq!((i as u8), *by);
                }
                rgr.release();
            };
        }

        assert!(ctr > 1);
    }

    #[test_log::test]
    fn frame_overcommit() {
        let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 3> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();

        // Create largeish grants
        let mut wgr = publisher.grant(128).unwrap();
        assert_eq!(wgr.len(), 128);
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = i as u8;
        }
        wgr.commit(255);

        let mut wgr = publisher.grant(64).unwrap();
        assert_eq!(wgr.len(), 64);
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = (i as u8) + 128;
        }
        wgr.commit(127);

        let rgr = subscriber.read().unwrap();
        assert_eq!(rgr.len(), 128);
        rgr.release();

        let rgr = subscriber.read().unwrap();
        assert_eq!(rgr.len(), 64);
        rgr.release();
    }

    #[test_log::test]
    fn frame_undercommit() {
        let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<512>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());

        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();

        for _ in 0..100 {
            // Create largeish grants
            let mut wgr = publisher.grant(128).unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = i as u8;
            }
            wgr.commit(13);

            let mut wgr = publisher.grant(64).unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = (i as u8) + 128;
            }
            wgr.commit(7);

            let mut wgr = publisher.grant(32).unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = (i as u8) + 192;
            }
            wgr.commit(0);

            let rgr = subscriber.read().unwrap();
            assert_eq!(rgr.len(), 13);
            rgr.release();

            let rgr = subscriber.read().unwrap();
            assert_eq!(rgr.len(), 7);
            rgr.release();

            let rgr = subscriber.read().unwrap();
            assert_eq!(rgr.len(), 0);
            rgr.release();
        }
    }

    #[test_log::test]
    fn frame_auto_commit_release() {
        let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();

        for _ in 0..100 {
            {
                let mut wgr = publisher.grant(64).unwrap();
                wgr.to_commit(64);
                assert_eq!(wgr.len(), 64);
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = i as u8;
                }
                // drop
            }

            {
                let mut rgr = subscriber.read().unwrap();
                rgr.auto_release(true);
                assert_eq!(rgr.len(), 64);
                for (i, by) in rgr.iter().enumerate() {
                    assert_eq!(*by, i as u8);
                }
                // drop
            }
        }

        assert!(subscriber.read().is_err());
    }

    #[test_log::test]
    fn async_frame_wrong_size() {
        block_on(async {
            let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 1> =
                PubSubChannel::new(StaticBufferProvider::new());
            let mut publisher = pubsub.publisher().unwrap();
            let mut subscriber = pubsub.subscriber().unwrap();

            // Create largeish grants
            let mut wgr = publisher.grant_async(127).await.unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = i as u8;
            }

            wgr.commit(256);

            let rgr = subscriber.read_async().await.unwrap();
            assert_eq!(rgr.len(), 127);
            for (i, by) in rgr.iter().enumerate() {
                assert_eq!((i as u8), *by);
            }
            rgr.release();
        });
    }

    #[test_log::test]
    fn async_frame_overcommit() {
        block_on(async {
            let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 1> =
                PubSubChannel::new(StaticBufferProvider::new());
            let mut publisher = pubsub.publisher().unwrap();
            let mut subscriber = pubsub.subscriber().unwrap();

            // Create largeish grants
            let mut wgr = publisher.grant_async(128).await.unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = i as u8;
            }
            wgr.commit(255);

            let mut wgr = publisher.grant_async(64).await.unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = (i as u8) + 128;
            }
            wgr.commit(127);

            let rgr = subscriber.read_async().await.unwrap();
            assert_eq!(rgr.len(), 128);
            rgr.release();

            let rgr = subscriber.read_async().await.unwrap();
            assert_eq!(rgr.len(), 64);
            rgr.release();
        });
    }

    #[test_log::test]
    fn async_frame_undercommit() {
        block_on(async {
            let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<512>, 1> =
                PubSubChannel::new(StaticBufferProvider::new());
            let mut publisher = pubsub.publisher().unwrap();
            let mut subscriber = pubsub.subscriber().unwrap();

            for _ in 0..100 {
                // Create largeish grants
                let mut wgr = publisher.grant_async(128).await.unwrap();
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = i as u8;
                }
                wgr.commit(13);

                let mut wgr = publisher.grant_async(64).await.unwrap();
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = (i as u8) + 128;
                }
                wgr.commit(7);

                let mut wgr = publisher.grant_async(32).await.unwrap();
                for (i, by) in wgr.iter_mut().enumerate() {
                    *by = (i as u8) + 192;
                }
                wgr.commit(0);

                let rgr = subscriber.read_async().await.unwrap();
                assert_eq!(rgr.len(), 13);
                rgr.release();

                let rgr = subscriber.read_async().await.unwrap();
                assert_eq!(rgr.len(), 7);
                rgr.release();

                let rgr = subscriber.read_async().await.unwrap();
                assert_eq!(rgr.len(), 0);
                rgr.release();
            }
        });
    }

    #[test_log::test]
    fn async_frame_auto_commit_release() {
        block_on(async {
            let pubsub: PubSubChannel<CriticalSectionRawMutex, StaticBufferProvider<256>, 1> =
                PubSubChannel::new(StaticBufferProvider::new());
            let mut publisher = pubsub.publisher().unwrap();
            let mut subscriber = pubsub.subscriber().unwrap();

            for _ in 0..100 {
                {
                    let mut wgr = publisher.grant_async(64).await.unwrap();
                    wgr.to_commit(64);
                    for (i, by) in wgr.iter_mut().enumerate() {
                        *by = i as u8;
                    }
                    // drop
                }

                {
                    let mut rgr = subscriber.read_async().await.unwrap();
                    rgr.auto_release(true);
                    let rgr = rgr;

                    for (i, by) in rgr.iter().enumerate() {
                        assert_eq!(*by, i as u8);
                    }
                    assert_eq!(rgr.len(), 64);
                    // drop
                }
            }

            assert!(subscriber.read().is_err());
        });
    }
}
