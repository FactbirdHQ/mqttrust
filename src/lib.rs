#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![allow(async_fn_in_trait)]

mod fmt;

#[cfg(all(feature = "mqttv3", feature = "mqttv5"))]
compile_error!("You must enable at most one of the following features: mqttv3, mqttv5");

mod broker;
mod client;
mod config;
mod encoding;
mod error;
mod packet;
// mod persistence;
pub mod pubsub;
mod stack;
mod state;

use core::{cell::RefCell, mem::MaybeUninit};

pub use client::MqttClient;
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};

pub use broker::{Broker, DomainBroker, IpBroker};
pub use config::Config;
use embedded_nal_async::TcpConnect;
use encoding::{PacketType, Pid, QoS};
use pubsub::{PubSubChannel, SliceBufferProvider};
pub use stack::MqttStack;
use state::Shared;

#[cfg(feature = "mqttv5")]
pub use encoding::{Properties, Property, RetainHandling};

#[cfg(feature = "max_topic_len_64")]
pub const MAX_TOPIC_LEN: usize = 64;

#[cfg(any(
    not(any(
        feature = "max_topic_len_64",
        feature = "max_topic_len_128",
        feature = "max_topic_len_256"
    )),
    feature = "max_topic_len_128"
))]
pub const MAX_TOPIC_LEN: usize = 128;

#[cfg(feature = "max_topic_len_256")]
pub const MAX_TOPIC_LEN: usize = 256;

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

// FIXME: don't hand-roll these serializations?
struct TxHeader {
    typ: PacketType,
    qos: Option<QoS>,
    pid: Pid,
}

impl TxHeader {
    fn from_bytes(bytes: &[u8]) -> (Self, &[u8]) {
        (
            Self {
                typ: PacketType::try_from(bytes[0]).unwrap(),
                qos: QoS::try_from(bytes[1]).ok(),
                pid: Pid::try_from(u16::from_le_bytes(bytes[2..3].try_into().unwrap())).unwrap(),
            },
            &bytes[4..],
        )
    }

    fn to_bytes(&self, buf: &mut [u8]) -> usize {
        buf[0] = u8::from(self.typ);
        buf[1] = match self.qos {
            Some(qos) => u8::from(qos),
            None => 0xFF,
        };
        buf[2..3].copy_from_slice(&self.pid.get().to_le_bytes()[..]);

        4
    }
}
