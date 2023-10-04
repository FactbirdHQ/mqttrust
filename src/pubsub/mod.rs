//! Implementation of [PubSubChannel], a queue where published messages get received by all subscribers.

use core::cell::{RefCell, UnsafeCell};
use core::cmp::min;
use core::fmt::Debug;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{self, AtomicBool, AtomicUsize, Ordering};
use core::task::{Context, Poll};

use futures::Future;

use self::publisher::GrantW;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::waitqueue::{MultiWakerRegistration, WakerRegistration};

// pub mod framed;
pub mod publisher;
// pub mod subscriber;

pub use publisher::Publisher;
// pub use subscriber::Subscriber;

pub struct PubSubChannel<M: RawMutex, const CAP: usize, const SUBS: usize> {
    inner: Mutex<M, RefCell<PubSubState<CAP, SUBS>>>,
}

impl<M: RawMutex, const CAP: usize, const SUBS: usize> PubSubChannel<M, CAP, SUBS> {
    /// Create a new channel
    pub const fn new() -> Self {
        Self {
            inner: Mutex::const_new(M::INIT, RefCell::new(PubSubState::new())),
        }
    }

    // /// Create a new subscriber. It will only receive messages that are published after its creation.
    // ///
    // /// If there are no subscriber slots left, an error will be returned.
    // pub fn subscriber(&self) -> Result<Subscriber<M, CAP, SUBS>, Error> {
    //     self.inner.lock(|inner| {
    //         let mut s = inner.borrow_mut();

    //         if s.subscriber_count >= SUBS {
    //             Err(Error::MaximumSubscribersReached)
    //         } else {
    //             s.subscriber_count += 1;
    //             Ok(Subscriber(Sub::new(s.next_message_id, self)))
    //         }
    //     })
    // }

    // fn read_with_context(&self, cx: Option<&mut Context<'_>>) -> Poll<GrantR<'_>> {
    //     self.inner.lock(|s| {
    //         let mut s = s.borrow_mut();

    //         // Check if we can read a message
    //         match s.get_message(*next_message_id) {
    //             // Yes, so we are done polling
    //             Some(WaitResult::Message(message)) => {
    //                 *next_message_id += 1;
    //                 Poll::Ready(WaitResult::Message(message))
    //             }
    //             // No, so we need to reregister our waker and sleep again
    //             None => {
    //                 if let Some(cx) = cx {
    //                     s.subscriber_wakers.register(cx.waker());
    //                 }
    //                 Poll::Pending
    //             }
    //             // We missed a couple of messages. We must do our internal bookkeeping and return that we lagged
    //             Some(WaitResult::Lagged(amount)) => {
    //                 *next_message_id += amount;
    //                 Poll::Ready(WaitResult::Lagged(amount))
    //             }
    //         }
    //     })
    // }

    // fn available(&self, next_message_id: u64) -> u64 {
    //     self.inner.lock(|s| s.borrow().next_message_id - next_message_id)
    // }

    fn grant_exact_with_context(
        &self,
        sz: usize,
        cx: Option<&mut Context<'_>>,
    ) -> Result<GrantW<'_, CAP, SUBS>, Error> {
        self.inner.lock(|s| {
            let mut s = s.borrow_mut();
            // Try to publish the message
            match s.try_grant_exact(sz) {
                // We did it, we are ready
                Ok(grant) => Ok(grant),
                // The queue is full, so we need to reregister our waker and go to sleep
                Err(message) => {
                    if let Some(cx) = cx {
                        s.publisher_waker.register(cx.waker());
                    }
                    Err(message)
                }
            }
        })
    }

    // fn space(&self) -> usize {
    //     self.inner.lock(|s| {
    //         let s = s.borrow();
    //         s.queue.capacity() - s.queue.len()
    //     })
    // }

    /// Create a new publisher
    ///
    /// If there are no publisher slots left, an error will be returned.
    pub fn publisher(&self) -> Result<Publisher<M, CAP, SUBS>, Error> {
        self.inner.lock(|inner| {
            let mut s = inner.borrow_mut();

            if s.publisher_taken.swap(true, Ordering::AcqRel) {
                return Err(Error::PublisherAlreadyTaken);
            }

            Ok(Publisher::new(self))
        })
    }

    // fn unregister_subscriber(&self, subscriber_next_message_id: u64) {
    //     self.inner.lock(|s| {
    //         let mut s = s.borrow_mut();
    //         s.unregister_subscriber(subscriber_next_message_id)
    //     })
    // }

    fn unregister_publisher(&self) {
        self.inner.lock(|s| {
            let mut s = s.borrow_mut();
            s.publisher_taken.store(false, Ordering::AcqRel);
        })
    }
}

/// Internal state for the PubSub channel
struct PubSubState<const CAP: usize, const SUBS: usize> {
    /// The queue contains the last messages that have been published and a countdown of how many subscribers are yet to read it
    buf: UnsafeCell<[u8; CAP]>,
    /// Every message has an id.
    /// Don't worry, we won't run out.
    /// If a million messages were published every second, then the ID's would run out in about 584942 years.
    next_message_id: u64,
    /// Collection of wakers for Subscribers that are waiting.  
    subscriber_wakers: MultiWakerRegistration<SUBS>,
    /// Collection of wakers for Publishers that are waiting.  
    publisher_waker: WakerRegistration,
    /// The amount of subscribers that are active
    subscriber_count: usize,

    /// Where the next byte will be written
    write: AtomicUsize,

    /// Where the next byte will be read from
    read: AtomicUsize,

    /// Used in the inverted case to mark the end of the
    /// readable streak. Otherwise will == sizeof::<self.buf>().
    /// Writer is responsible for placing this at the correct
    /// place when entering an inverted condition, and Reader
    /// is responsible for moving it back to sizeof::<self.buf>()
    /// when exiting the inverted condition
    last: AtomicUsize,

    /// Used by the Writer to remember what bytes are currently
    /// allowed to be written to, but are not yet ready to be
    /// read from
    reserve: AtomicUsize,

    /// Is there an active read grant?
    read_in_progress: AtomicBool,

    /// Is there an active write grant?
    write_in_progress: AtomicBool,

    publisher_taken: AtomicBool,
}

impl<const CAP: usize, const SUBS: usize> PubSubState<CAP, SUBS> {
    /// Create a new internal channel state
    const fn new() -> Self {
        Self {
            buf: UnsafeCell::new([0; CAP]),
            next_message_id: 0,
            subscriber_wakers: MultiWakerRegistration::new(),
            publisher_waker: WakerRegistration::new(),
            subscriber_count: 0,

            /// Owned by the writer
            write: AtomicUsize::new(0),

            /// Owned by the reader
            read: AtomicUsize::new(0),

            /// Cooperatively owned
            ///
            /// NOTE: This should generally be initialized as size_of::<self.buf>(), however
            /// this would prevent the structure from being entirely zero-initialized,
            /// and can cause the .data section to be much larger than necessary. By
            /// forcing the `last` pointer to be zero initially, we place the structure
            /// in an "inverted" condition, which will be resolved on the first commited
            /// bytes that are written to the structure.
            ///
            /// When read == last == write, no bytes will be allowed to be read (good), but
            /// write grants can be given out (also good).
            last: AtomicUsize::new(0),

            /// Owned by the Writer, "private"
            reserve: AtomicUsize::new(0),

            /// Owned by the Reader, "private"
            read_in_progress: AtomicBool::new(false),

            /// Owned by the Writer, "private"
            write_in_progress: AtomicBool::new(false),

            publisher_taken: AtomicBool::new(false),
        }
    }

    fn try_grant_exact(&mut self, sz: usize) -> Result<GrantW<'_, CAP, SUBS>, Error> {
        if self.write_in_progress.swap(true, Ordering::AcqRel) {
            return Err(Error::GrantInProgress);
        }

        // Writer component. Must never write to `read`,
        // be careful writing to `load`
        let write = self.write.load(Ordering::Acquire);
        let read = self.read.load(Ordering::Acquire);
        let already_inverted = write < read;

        let start = if already_inverted {
            if (write + sz) < read {
                // Inverted, room is still available
                write
            } else {
                // Inverted, no room is available
                self.write_in_progress.store(false, Ordering::Release);
                return Err(Error::InsufficientSize);
            }
        } else {
            if write + sz <= CAP {
                // Non inverted condition
                write
            } else {
                // Not inverted, but need to go inverted

                // NOTE: We check sz < read, NOT <=, because
                // write must never == read in an inverted condition, since
                // we will then not be able to tell if we are inverted or not
                if sz < read {
                    // Invertible situation
                    0
                } else {
                    // Not invertible, no space
                    self.write_in_progress.store(false, Ordering::Release);
                    return Err(Error::InsufficientSize);
                }
            }
        };

        // Safe write, only viewed by this task
        self.reserve.store(start + sz, Ordering::Release);

        // This is sound, as UnsafeCell, MaybeUninit, and GenericArray
        // are all `#[repr(Transparent)]
        let start_of_buf_ptr = unsafe { (&mut *self.buf.get()).as_mut_ptr().cast::<u8>() };
        let grant_slice =
            unsafe { core::slice::from_raw_parts_mut(start_of_buf_ptr.offset(start as isize), sz) };

        // FIXME: Move this optimization to the actual GrantW
        // if self.subscriber_count == 0 {
        //     // We don't need to publish anything because there is no one to receive it
        //     return Ok(());
        // }

        // // Wake all of the subscribers
        // self.subscriber_wakers.wake();

        Ok(GrantW {
            buf: grant_slice,
            pbs: self,
            to_commit: 0,
        })
    }

    // fn get_message(&mut self, message_id: u64) -> Option<WaitResult> {
    //     let start_id = self.next_message_id - self.queue.len() as u64;

    //     if message_id < start_id {
    //         return Some(WaitResult::Lagged(start_id - message_id));
    //     }

    //     let current_message_index = (message_id - start_id) as usize;

    //     if current_message_index >= self.queue.len() {
    //         return None;
    //     }

    //     // We've checked that the index is valid
    //     let queue_item = self.queue.iter_mut().nth(current_message_index).unwrap();

    //     // We're reading this item, so decrement the counter
    //     queue_item.1 -= 1;

    //     let message = if current_message_index == 0 && queue_item.1 == 0 {
    //         let (message, _) = self.queue.pop_front().unwrap();
    //         self.publisher_wakers.wake();
    //         // Return pop'd message without clone
    //         message
    //     } else {
    //         queue_item.0.clone()
    //     };

    //     Some(WaitResult::Message(message))
    // }

    // fn unregister_subscriber(&mut self, subscriber_next_message_id: u64) {
    //     self.subscriber_count -= 1;

    //     // All messages that haven't been read yet by this subscriber must have their counter decremented
    //     let start_id = self.next_message_id - self.queue.len() as u64;
    //     if subscriber_next_message_id >= start_id {
    //         let current_message_index = (subscriber_next_message_id - start_id) as usize;
    //         self.queue
    //             .iter_mut()
    //             .skip(current_message_index)
    //             .for_each(|(_, counter)| *counter -= 1);

    //         let mut wake_publishers = false;
    //         while let Some((_, count)) = self.queue.front() {
    //             if *count == 0 {
    //                 self.queue.pop_front().unwrap();
    //                 wake_publishers = true;
    //             } else {
    //                 break;
    //             }
    //         }

    //         if wake_publishers {
    //             self.publisher_wakers.wake();
    //         }
    //     }
    // }
}

/// Error type for the [PubSubChannel]
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    /// All subscriber slots are used. To add another subscriber, first another subscriber must be dropped or
    /// the capacity of the channels must be increased.
    MaximumSubscribersReached,

    GrantInProgress,

    InsufficientSize,

    PublisherAlreadyTaken,
}
