mod publisher;
mod state;
mod subscriber;

use core::cell::RefCell;

use embassy_sync::blocking_mutex::{
    raw::{NoopRawMutex, RawMutex},
    Mutex,
};
pub use publisher::*;
pub use subscriber::*;

mod buffer_provider;
pub use buffer_provider::*;

use self::framed::{FramePublisher, FrameSubscriber};

pub mod framed;
mod vusize;

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

    PublisherAlreadyTaken,

    MaximumSubscribersReached,
}

pub struct PubSubChannel<B: BufferProvider, const SUBS: usize> {
    inner: Mutex<NoopRawMutex, RefCell<state::PubSubState<B, SUBS>>>,
}

impl<B: BufferProvider, const SUBS: usize> PubSubChannel<B, SUBS> {
    pub fn new(buf: B) -> Self {
        Self {
            inner: Mutex::const_new(
                NoopRawMutex::INIT,
                RefCell::new(state::PubSubState::new(buf)),
            ),
        }
    }

    /// Create a new `Subscriber`. It will only receive messages that are published after its creation.
    ///
    /// If there are no subscriber slots left, an error will be returned.
    pub fn subscriber(&self) -> Result<Subscriber<'_, B, SUBS>> {
        self.inner.lock(|inner| {
            let mut s = inner.borrow_mut();

            if s.subscriber_count >= SUBS {
                Err(Error::MaximumSubscribersReached)
            } else {
                s.subscriber_count += 1;
                Ok(Subscriber::new(0, &self.inner))
            }
        })
    }

    /// Create a new `FrameSubscriber`. It will only receive messages that are published after its creation.
    ///
    /// If there are no subscriber slots left, an error will be returned.
    pub fn framed_subscriber(&self) -> Result<FrameSubscriber<'_, B, SUBS>> {
        self.inner.lock(|inner| {
            let mut s = inner.borrow_mut();

            if s.subscriber_count >= SUBS {
                Err(Error::MaximumSubscribersReached)
            } else {
                s.subscriber_count += 1;
                Ok(FrameSubscriber::new(0, &self.inner))
            }
        })
    }

    /// Create a new `Publisher`.
    ///
    /// If a publisher has already been taken, an error will be returned.
    pub fn publisher(&self) -> Result<Publisher<'_, B, SUBS>> {
        self.inner.lock(|inner| {
            let mut s = inner.borrow_mut();

            if s.publisher_taken {
                Err(Error::PublisherAlreadyTaken)
            } else {
                s.publisher_taken = true;
                Ok(Publisher::new(&self.inner))
            }
        })
    }

    /// Create a new `Publisher`.
    ///
    /// If a publisher has already been taken, an error will be returned.
    pub fn framed_publisher(&self) -> Result<FramePublisher<'_, B, SUBS>> {
        self.inner.lock(|inner| {
            let mut s = inner.borrow_mut();

            if s.publisher_taken {
                Err(Error::PublisherAlreadyTaken)
            } else {
                s.publisher_taken = true;
                Ok(FramePublisher::new(&self.inner))
            }
        })
    }
}
