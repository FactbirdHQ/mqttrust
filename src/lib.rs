#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![allow(async_fn_in_trait)]
mod fmt;
pub mod transport;

#[cfg(all(feature = "mqttv3", feature = "mqttv5"))]
compile_error!("You must enable at most one of the following features: mqttv3, mqttv5");

#[cfg(not(any(feature = "mqttv3", feature = "mqttv5")))]
compile_error!("You must enable one of the following features: mqttv3, mqttv5");

mod broker;
mod client;
mod config;
mod encoding;
mod error;
mod packet;
mod stack;
mod state;
mod topic_filter;

#[cfg(feature = "_test")]
pub mod pubsub;

#[cfg(not(feature = "_test"))]
mod pubsub;

use core::mem::MaybeUninit;

pub use bitmaps::{Bits, BitsImpl};
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};

pub use broker::{Broker, DomainBroker, IpBroker};
pub use client::{MqttClient, Subscription};
pub use config::Config;
pub use pubsub::{
    BufferProvider, FrameGrantR, FrameGrantW, FramePublisher, FrameSubscriber, Message,
    PubSubChannel, SliceBufferProvider,
};
pub use stack::MqttStack;
use state::Shared;

pub use encoding::*;
pub use error::Error;

#[cfg(feature = "mqttv5")]
pub use encoding::{Properties, Property, RetainHandling};

pub struct State<M: RawMutex, const TX: usize, const RX: usize, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
{
    tx: [u8; TX],
    rx: [u8; RX],
    inner: MaybeUninit<StateInner<'static, M, SUBS>>,
}

impl<M: RawMutex, const TX: usize, const RX: usize, const SUBS: usize> Default
    for State<M, TX, RX, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<M: RawMutex, const TX: usize, const RX: usize, const SUBS: usize> State<M, TX, RX, SUBS>
where
    BitsImpl<{ SUBS }>: Bits,
{
    pub const fn new() -> Self {
        Self {
            tx: [0; TX],
            rx: [0; RX],
            inner: MaybeUninit::uninit(),
        }
    }
}

struct StateInner<'a, M: RawMutex, const SUBS: usize>
where
    BitsImpl<{ SUBS }>: Bits,
{
    pub(crate) tx: PubSubChannel<SliceBufferProvider<'a>, 1>,
    pub(crate) rx_pubsub: PubSubChannel<SliceBufferProvider<'a>, SUBS>,
    pub(crate) shared: Mutex<M, Shared<SUBS>>,
}

pub fn new<'d, M: RawMutex, const TX: usize, const RX: usize, const SUBS: usize>(
    state: &'d mut State<M, TX, RX, SUBS>,
    config: Config,
) -> (MqttStack<'d, M, SUBS>, MqttClient<'d, M, SUBS>)
where
    BitsImpl<{ SUBS }>: Bits,
{
    // safety: this is a self-referential struct, however:
    // - it can't move while the `'d` borrow is active.
    // - when the borrow ends, the dangling references inside the MaybeUninit will never be used again.
    let state_uninit: *mut MaybeUninit<StateInner<'d, M, SUBS>> =
        (&mut state.inner as *mut MaybeUninit<StateInner<'static, M, SUBS>>).cast();

    let state = unsafe { &mut *state_uninit }.write(StateInner {
        tx: PubSubChannel::<_, 1>::new(SliceBufferProvider::new(&mut state.tx[..])),
        rx_pubsub: PubSubChannel::<_, SUBS>::new(SliceBufferProvider::new(&mut state.rx[..])),
        shared: Mutex::new(Shared::new()),
    });

    let client_id = config.client_id.clone();

    (
        MqttStack::new(
            config,
            &state.shared,
            state.tx.subscriber().unwrap(),
            state.rx_pubsub.publisher().unwrap(),
        ),
        MqttClient::new(
            state.tx.publisher().unwrap(),
            &state.shared,
            &state.rx_pubsub,
            client_id,
        ),
    )
}

#[cfg(not(test))]
pub mod crate_config {
    #![allow(unused)]
    include!(concat!(env!("OUT_DIR"), "/config.rs"));
}

#[cfg(test)]
pub mod crate_config {
    #![allow(unused)]
    pub const MAX_CLIENT_ID_LEN: usize = 128;
    pub const MAX_TOPIC_LEN: usize = 128;
    pub const MAX_SUB_TOPICS_PER_MSG: usize = 8;
    pub const MAX_INFLIGHT: usize = 8;
}
