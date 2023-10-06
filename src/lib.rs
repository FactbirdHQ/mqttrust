#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![feature(async_fn_in_trait)]
#![feature(type_alias_impl_trait)]

mod broker;
mod client;
mod config;
mod encoding;
mod packet;
mod error;
pub mod pubsub;
mod reason_codes;
mod stack;
mod state;

use core::{cell::RefCell, mem::MaybeUninit};

use client::MqttClient;
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};
use embedded_io_async::{Read, Write};

pub use broker::{Broker, DomainBroker, IpBroker};
pub use config::Config;
use pubsub::{PubSubChannel, SliceBufferProvider};
use stack::MqttStack;
use state::Shared;

pub struct State<M: RawMutex, const TX: usize, const RX: usize> {
    tx: [u8; TX],
    rx: [u8; RX],
    inner: MaybeUninit<StateInner<'static, M>>,
}

impl<M: RawMutex, const TX: usize, const RX: usize> State<M, TX, RX> {
    pub const fn new() -> Self {
        Self {
            tx: [0; TX],
            rx: [0; RX],
            inner: MaybeUninit::uninit(),
        }
    }
}

pub const SUBS: usize = 3;

struct StateInner<'a, M: RawMutex> {
    pub(crate) tx: PubSubChannel<M, SliceBufferProvider<'a>, 1>,
    pub(crate) rx_pubsub: PubSubChannel<M, SliceBufferProvider<'a>, SUBS>,
    pub(crate) shared: Mutex<M, RefCell<Shared>>,
}

pub fn new<'d, M: RawMutex, B: Broker, S: Read + Write, const TX: usize, const RX: usize>(
    state: &'d mut State<M, TX, RX>,
    config: Config<B>,
    socket: S,
) -> (MqttStack<'d, M, B, S>, MqttClient<'d, M>) {
    // safety: this is a self-referential struct, however:
    // - it can't move while the `'d` borrow is active.
    // - when the borrow ends, the dangling references inside the MaybeUninit will never be used again.
    let state_uninit: *mut MaybeUninit<StateInner<'d, M>> =
        (&mut state.inner as *mut MaybeUninit<StateInner<'static, M>>).cast();

    let state = unsafe { &mut *state_uninit }.write(StateInner {
        tx: PubSubChannel::new(SliceBufferProvider::new(&mut state.tx[..])),
        rx_pubsub: PubSubChannel::new(SliceBufferProvider::new(&mut state.rx[..])),
        shared: Mutex::new(RefCell::new(Shared::new())),
    });

    (
        MqttStack::new(
            socket,
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
