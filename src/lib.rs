#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![feature(async_fn_in_trait)]
#![feature(type_alias_impl_trait)]

mod fmt;

mod broker;
mod client;
mod config;
#[cfg_attr(feature = "mqttv3", path = "encoding/v4/mod.rs")]
#[cfg_attr(feature = "mqttv5", path = "encoding/v5/mod.rs")]
mod encoding;
mod error;
mod packet;
pub mod pubsub;
mod reason_codes;
mod stack;
mod state;

use core::{cell::RefCell, mem::MaybeUninit};

pub use client::MqttClient;
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};

pub use broker::{Broker, DomainBroker, IpBroker};
pub use config::Config;
use embedded_nal_async::TcpConnect;
use pubsub::{PubSubChannel, SliceBufferProvider};
pub use stack::MqttStack;
use state::Shared;

pub use encoding::{Packet, Publish, QoS, Subscribe, SubscribeTopic};

pub struct State<M: RawMutex, const TX: usize, const RX: usize, const SUBS: usize> {
    tx: [u8; TX],
    rx: [u8; RX],
    inner: MaybeUninit<StateInner<'static, M, SUBS>>,
}

impl<M: RawMutex, const TX: usize, const RX: usize, const SUBS: usize> State<M, TX, RX, SUBS> {
    pub const fn new() -> Self {
        Self {
            tx: [0; TX],
            rx: [0; RX],
            inner: MaybeUninit::uninit(),
        }
    }
}

struct StateInner<'a, M: RawMutex, const SUBS: usize> {
    pub(crate) tx: PubSubChannel<M, SliceBufferProvider<'a>, 1>,
    pub(crate) rx_pubsub: PubSubChannel<M, SliceBufferProvider<'a>, SUBS>,
    pub(crate) shared: Mutex<M, RefCell<Shared<SUBS>>>,
}

pub fn new<
    'd,
    M: RawMutex,
    B: Broker,
    N: TcpConnect,
    const TX: usize,
    const RX: usize,
    const SUBS: usize,
>(
    state: &'d mut State<M, TX, RX, SUBS>,
    config: Config<B>,
    network: &'d N,
) -> (MqttStack<'d, M, B, N, SUBS>, MqttClient<'d, M, SUBS>) {
    // safety: this is a self-referential struct, however:
    // - it can't move while the `'d` borrow is active.
    // - when the borrow ends, the dangling references inside the MaybeUninit will never be used again.
    let state_uninit: *mut MaybeUninit<StateInner<'d, M, SUBS>> =
        (&mut state.inner as *mut MaybeUninit<StateInner<'static, M, SUBS>>).cast();

    let state = unsafe { &mut *state_uninit }.write(StateInner {
        tx: PubSubChannel::new(SliceBufferProvider::new(&mut state.tx[..])),
        rx_pubsub: PubSubChannel::new(SliceBufferProvider::new(&mut state.rx[..])),
        shared: Mutex::new(RefCell::new(Shared::new())),
    });

    (
        MqttStack::new(
            network,
            config,
            &state.shared,
            state.tx.framed_subscriber().unwrap(),
            state.rx_pubsub.framed_publisher().unwrap(),
        ),
        MqttClient::new(
            state.tx.framed_publisher().unwrap(),
            &state.shared,
            &state.rx_pubsub,
        ),
    )
}
