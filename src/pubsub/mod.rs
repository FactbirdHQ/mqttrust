//! Publish/Subscribe queue directly target towards internal usage in an MQTT
//! client implementation.
//!
//! This implementation is highly inspired by both `embassy_sync::Channel`, and
//! `bbqueue`. Full credit to those projects for the complex internals.

mod atomic_multiwakers;
pub(crate) mod header;
mod message;
mod publisher;
mod subscriber;

use bitmaps::{Bitmap, Bits, BitsImpl};
use core::cell::{RefCell, UnsafeCell};
use portable_atomic::{AtomicBool, AtomicUsize, Ordering::AcqRel};

use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
pub use message::*;
pub use publisher::*;
pub use subscriber::*;

mod buffer_provider;
pub use buffer_provider::*;

/// Result type used by the `BBQueue` interfaces
pub type Result<T> = core::result::Result<T, Error>;

/// Error type used by the `BBQueue` interfaces
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Error {
    /// The buffer does not contain sufficient size for the requested action
    InsufficientSize,

    /// Unable to produce another grant, a grant of this type is already in
    /// progress
    GrantInProgress,

    MismatchedTopicFilter,

    PublisherAlreadyTaken,

    MaximumSubscribersReached,
}

pub struct PubSubChannel<B: BufferProvider, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
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

    /// Collection of wakers for [`Subscriber`]'s that are waiting.
    pub(crate) subscriber_wakers: atomic_multiwakers::MultiWakerRegistration<SUBS>,

    pub(crate) subscribers_taken: Mutex<CriticalSectionRawMutex, RefCell<Bitmap<SUBS>>>,
    pub(crate) message_bitmap: Mutex<CriticalSectionRawMutex, RefCell<Bitmap<SUBS>>>,
    pub(crate) publisher_taken: AtomicBool,

    /// Write waker for async support
    /// Woken up when a release is done
    pub(crate) publisher_waker: atomic_waker::AtomicWaker,
}

impl<B: BufferProvider, const SUBS: usize> PubSubChannel<B, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
{
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
            publisher_waker: atomic_waker::AtomicWaker::new(),
            subscribers_taken: Mutex::new(RefCell::new(Bitmap::new())),
            message_bitmap: Mutex::new(RefCell::new(Bitmap::new())),
            subscriber_wakers: atomic_multiwakers::MultiWakerRegistration::new(),
        }
    }

    /// Create a new `FrameSubscriber`. It will only receive messages that are published after its creation.
    ///
    /// If there are no subscriber slots left, an error will be returned.
    pub fn subscriber(&self) -> Result<FrameSubscriber<'_, B, SUBS>> {
        self.subscribers_taken.lock(|f| {
            let mut map = f.borrow_mut();
            if let Some(id) = map.first_false_index() {
                map.set(id, true);
                Ok(FrameSubscriber::new(self, id))
            } else {
                Err(Error::MaximumSubscribersReached)
            }
        })
    }

    /// Create a new `FramePublisher`.
    ///
    /// If a publisher has already been taken, an error will be returned.
    pub fn publisher(&self) -> Result<FramePublisher<'_, B, SUBS>> {
        if self.publisher_taken.swap(true, AcqRel) {
            return Err(Error::PublisherAlreadyTaken);
        }

        unsafe {
            // Explicitly zero the data to avoid undefined behavior.
            // This is required, because we hand out references to the buffers,
            // which mean that creating them as references is technically UB for now
            let mu_ptr = (*self.buf.get()).buf();
            core::ptr::write_bytes(mu_ptr.as_mut_ptr(), 0, mu_ptr.len());
        }

        Ok(FramePublisher::new(self))
    }
}

impl<'a, const SUBS: usize> PubSubChannel<SliceBufferProvider<'a>, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
{
    pub fn new_from_slice(buf: &'a mut [u8]) -> Self {
        Self::new(SliceBufferProvider::new(buf))
    }
}

mod atomic_waker {
    use core::cell::Cell;
    use core::task::Waker;

    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use embassy_sync::blocking_mutex::Mutex;

    /// Utility struct to register and wake a waker.
    pub struct AtomicWaker {
        waker: Mutex<CriticalSectionRawMutex, Cell<Option<Waker>>>,
    }

    impl AtomicWaker {
        /// Create a new `AtomicWaker`.
        pub const fn new() -> Self {
            Self {
                waker: Mutex::const_new(CriticalSectionRawMutex::new(), Cell::new(None)),
            }
        }

        /// Register a waker. Overwrites the previous waker, if any.
        pub fn register(&self, w: &Waker) {
            self.waker.lock(|cell| {
                cell.set(match cell.replace(None) {
                    Some(w2) if (w2.will_wake(w)) => Some(w2),
                    Some(old_waker) => {
                        old_waker.wake();
                        Some(w.clone())
                    }
                    _ => Some(w.clone()),
                })
            })
        }

        /// Wake the registered waker, if any.
        pub fn wake(&self) {
            self.waker.lock(|cell| {
                if let Some(w) = cell.replace(None) {
                    w.wake_by_ref();
                    cell.set(Some(w));
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::pubsub::header::Header;

    use super::{PubSubChannel, StaticBufferProvider};
    use embassy_futures::block_on;

    #[test_log::test]
    fn frame_wrong_size() {
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();

        // Create largeish grants
        let mut wgr = publisher.grant(127).unwrap();
        for (i, by) in wgr.iter_mut().enumerate() {
            *by = i as u8;
        }
        // Note: In debug mode, this hits a debug_assert
        wgr.commit(256);

        let rgr = subscriber.read_any().unwrap();
        assert_eq!(rgr.len(), 127);
        for (i, by) in rgr.iter().enumerate() {
            assert_eq!((i as u8), *by);
        }
        rgr.release();
    }

    #[test_log::test]
    fn full_size() {
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
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

                let rgr = subscriber.read_any().unwrap();
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

                let rgr = subscriber.read_any().unwrap();
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
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 3> =
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

        let rgr = subscriber.read_any().unwrap();
        assert_eq!(rgr.len(), 128);
        rgr.release();

        let rgr = subscriber.read_any().unwrap();
        assert_eq!(rgr.len(), 64);
        rgr.release();
    }

    #[test_log::test]
    fn frame_undercommit() {
        let pubsub: PubSubChannel<StaticBufferProvider<512>, 1> =
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

            let rgr = subscriber.read_any().unwrap();
            assert_eq!(rgr.len(), 13);
            rgr.release();

            let rgr = subscriber.read_any().unwrap();
            assert_eq!(rgr.len(), 7);
            rgr.release();

            let rgr = subscriber.read_any().unwrap();
            assert_eq!(rgr.len(), 0);
            rgr.release();
        }
    }

    #[test_log::test]
    fn frame_auto_commit_release() {
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
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
                let mut rgr = subscriber.read_any().unwrap();
                rgr.auto_release(true);
                assert_eq!(rgr.len(), 64);
                for (i, by) in rgr.iter().enumerate() {
                    assert_eq!(*by, i as u8);
                }
                // drop
            }
        }

        assert!(subscriber.read_any().is_err());
    }

    #[test_log::test]
    fn async_frame_wrong_size() {
        block_on(async {
            let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
                PubSubChannel::new(StaticBufferProvider::new());
            let mut publisher = pubsub.publisher().unwrap();
            let mut subscriber = pubsub.subscriber().unwrap();

            // Create largeish grants
            let mut wgr = publisher.grant_async(127).await.unwrap();
            for (i, by) in wgr.iter_mut().enumerate() {
                *by = i as u8;
            }
            // Note: In debug mode, this hits a debug_assert
            wgr.commit(256);

            let rgr = subscriber.read_any_async().await.unwrap();
            assert_eq!(rgr.len(), 127);
            for (i, by) in rgr.iter().enumerate() {
                assert_eq!((i as u8), *by);
            }
            rgr.release();
        });
    }

    #[test_log::test]
    fn async_full_size() {
        block_on(async {
            let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
                PubSubChannel::new(StaticBufferProvider::new());
            let mut publisher = pubsub.publisher().unwrap();
            let mut subscriber = pubsub.subscriber().unwrap();

            let mut ctr = 0;

            for _ in 0..100 {
                // Create largeish grants
                if let Ok(mut wgr) = publisher.grant_async(127).await {
                    ctr += 1;
                    for (i, by) in wgr.iter_mut().enumerate() {
                        *by = i as u8;
                    }
                    wgr.commit(127);

                    let rgr = subscriber.read_any_async().await.unwrap();
                    assert_eq!(rgr.len(), 127);
                    for (i, by) in rgr.iter().enumerate() {
                        assert_eq!((i as u8), *by);
                    }
                    rgr.release();
                } else {
                    // Create smallish grants
                    let mut wgr = publisher.grant_async(1).await.unwrap();
                    for (i, by) in wgr.iter_mut().enumerate() {
                        *by = i as u8;
                    }
                    wgr.commit(1);

                    let rgr = subscriber.read_any_async().await.unwrap();
                    assert_eq!(rgr.len(), 1);
                    for (i, by) in rgr.iter().enumerate() {
                        assert_eq!((i as u8), *by);
                    }
                    rgr.release();
                };
            }

            assert!(ctr > 1);
        });
    }

    #[test_log::test]
    fn async_frame_overcommit() {
        block_on(async {
            let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
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

            let rgr = subscriber.read_any_async().await.unwrap();
            assert_eq!(rgr.len(), 128);
            rgr.release();

            let rgr = subscriber.read_any_async().await.unwrap();
            assert_eq!(rgr.len(), 64);
            rgr.release();
        });
    }

    #[test_log::test]
    fn async_frame_undercommit() {
        block_on(async {
            let pubsub: PubSubChannel<StaticBufferProvider<512>, 1> =
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

                let rgr = subscriber.read_any_async().await.unwrap();
                assert_eq!(rgr.len(), 13);
                rgr.release();

                let rgr = subscriber.read_any_async().await.unwrap();
                assert_eq!(rgr.len(), 7);
                rgr.release();

                let rgr = subscriber.read_any_async().await.unwrap();
                assert_eq!(rgr.len(), 0);
                rgr.release();
            }
        });
    }

    #[test_log::test]
    fn async_frame_auto_commit_release() {
        block_on(async {
            let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
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
                    let mut rgr = subscriber.read_any_async().await.unwrap();
                    rgr.auto_release(true);
                    let rgr = rgr;

                    for (i, by) in rgr.iter().enumerate() {
                        assert_eq!(*by, i as u8);
                    }
                    assert_eq!(rgr.len(), 64);
                    // drop
                }
            }

            assert!(subscriber.read_any().is_err());
        });
    }

    #[test_log::test]
    fn subscriber_bitmap() {
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 10> =
            PubSubChannel::new(StaticBufferProvider::new());
        {
            let _subscriber1 = pubsub.subscriber().unwrap();
            let _subscriber2 = pubsub.subscriber().unwrap();
            let _subscriber3 = pubsub.subscriber().unwrap();
            let _subscriber4 = pubsub.subscriber().unwrap();

            let map = pubsub.subscribers_taken.lock(|s| s.borrow().clone());
            assert_eq!(map.as_bytes()[0], 0b00001111);
            assert_eq!(map.as_bytes()[1], 0b00000000);

            drop(_subscriber3);

            let map = pubsub.subscribers_taken.lock(|s| s.borrow().clone());
            assert_eq!(map.as_bytes()[0], 0b00001011);
            assert_eq!(map.as_bytes()[1], 0b00000000);

            let _subscriber3 = pubsub.subscriber().unwrap();

            let map = pubsub.subscribers_taken.lock(|s| s.borrow().clone());
            assert_eq!(map.as_bytes()[0], 0b00001111);
            assert_eq!(map.as_bytes()[1], 0b00000000);
        }

        let map = pubsub.subscribers_taken.lock(|s| s.borrow().clone());
        assert_eq!(map.as_bytes()[0], 0b00000000);
        assert_eq!(map.as_bytes()[1], 0b00000000);
    }

    #[test_log::test]
    fn watermark_test_1() {
        // Test filling the buffer completely, then reading it completely
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();

        assert_eq!(publisher.free_capacity(), 256);

        assert_eq!(Header::encoded_len(254), 2);

        // Write a full buffer (254 bytes + 2 bytes header)
        let grant = publisher.grant(254).unwrap();
        grant.buf.fill(0xAA);
        grant.commit(254);

        // Read a full buffer
        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 254);
        assert!(grant.buf().iter().all(|&b| b == 0xAA));
        grant.release();

        // Buffer should be empty now
        assert_eq!(publisher.free_capacity(), 256);
    }

    #[test_log::test]
    fn watermark_test_2() {
        // Test writing and reading data in chunks smaller than the buffer size
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();

        // Write first chunk
        let grant = publisher.grant(100).unwrap();
        grant.buf.fill(0xAA);
        grant.commit(100);

        // Read first chunk
        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 100);
        assert!(grant.buf().iter().all(|&b| b == 0xAA));
        grant.release();

        // Write second chunk
        let grant = publisher.grant(50).unwrap();
        grant.buf.fill(0xBB);
        grant.commit(50);

        // Read second chunk
        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 50);
        assert!(grant.buf().iter().all(|&b| b == 0xBB));
        grant.release();

        // Write third chunk to fill the buffer
        let grant = publisher.grant(106).unwrap();
        grant.buf.fill(0xCC);
        grant.commit(106);

        // Read third chunk
        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 106);
        assert!(grant.buf().iter().all(|&b| b == 0xCC));
        grant.release();

        // Buffer should be empty now
        assert_eq!(publisher.free_capacity(), 256);
    }

    #[test_log::test]
    fn watermark_test_3() {
        // Test wrapping around the buffer
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();

        let grant = publisher.grant(200).unwrap();
        grant.buf.fill(0xAA);
        grant.commit(200);

        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 200);
        assert!(grant.buf()[..56].iter().all(|&b| b == 0xAA));
        grant.release();

        let grant = publisher.grant(200).unwrap();
        grant.buf.fill(0xBB);
        grant.commit(200);

        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 200);
        assert!(grant.buf().iter().all(|&b| b == 0xBB));
        grant.release();

        let grant = publisher.grant(50).unwrap();
        grant.buf.fill(0xCC);
        grant.commit(50);

        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 50);
        assert!(grant.buf().iter().all(|&b| b == 0xCC));
        grant.release();

        // Buffer should be empty now
        assert_eq!(publisher.free_capacity(), 256);
    }

    #[test_log::test]
    fn watermark_test_4() {
        // Test writing a chunk that exactly fills the buffer after wrapping
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();

        // Write first chunk
        assert_eq!(Header::encoded_len(100), 1);
        let grant = publisher.grant(100).unwrap();
        grant.buf.fill(0xAA);
        grant.commit(100);

        // Read part of the first chunk
        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 100);
        assert!(grant.buf().iter().all(|&b| b == 0xAA));
        grant.release();

        // Buffer should be empty now
        assert_eq!(publisher.free_capacity(), 256);

        // Write a chunk that exactly fills the buffer after wrapping
        assert_eq!(Header::encoded_len(153), 2);
        let grant = publisher.grant(153).unwrap();
        grant.buf.fill(0xBB);
        grant.commit(153);

        // Read the rest of the first chunk and the second chunk
        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 153);
        assert!(grant.buf().iter().all(|&b| b == 0xBB));
        grant.release();

        // Buffer should be empty now
        assert_eq!(publisher.free_capacity(), 256);
    }

    #[test_log::test]
    fn watermark_test_5() {
        // Test writing a chunk smaller than the buffer, then a chunk that fills the buffer exactly
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let mut subscriber = pubsub.subscriber().unwrap();

        // Write a small chunk
        let grant = publisher.grant(50).unwrap();
        grant.buf.fill(0xAA);
        grant.commit(50);

        // Write a chunk that fills the buffer exactly
        let grant = publisher.grant(203).unwrap();
        grant.buf.fill(0xBB);
        grant.commit(203);

        // Read the first chunk
        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 50);
        assert!(grant.buf().iter().all(|&b| b == 0xAA));
        grant.release();

        let grant = subscriber.read_any().unwrap();
        assert_eq!(grant.buf().len(), 203);
        assert!(grant.buf().iter().all(|&b| b == 0xBB));
        grant.release();

        // Buffer should be empty now
        assert_eq!(publisher.free_capacity(), 256);
    }

    #[test_log::test]
    fn watermark_test_6() {
        // Test writing a chunk larger than the buffer, then a smaller chunk
        let pubsub: PubSubChannel<StaticBufferProvider<256>, 1> =
            PubSubChannel::new(StaticBufferProvider::new());
        let mut publisher = pubsub.publisher().unwrap();
        let _subscriber = pubsub.subscriber().unwrap();

        // Write a chunk larger than the buffer
        assert!(publisher.grant(300).is_err());
    }
}
