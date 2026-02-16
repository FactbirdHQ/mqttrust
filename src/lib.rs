#![cfg_attr(all(not(test), not(feature = "std")), no_std)]
#![allow(async_fn_in_trait)]

//! A lightweight, `no_std` async MQTT client for embedded systems.
//!
//! This crate provides a split-architecture MQTT client designed for use with the
//! Embassy async ecosystem. The entry point is the [`new()`] function, which returns:
//!
//! - [`MqttStack`]: The protocol engine that must be driven in a long-running async task.
//!   It handles connection management, keep-alive, and packet encoding/decoding.
//! - [`MqttClient`]: A clonable handle used to publish messages and create subscriptions.
//!
//! # Example
//!
//! ```ignore
//! use embassy_sync::blocking_mutex::raw::NoopRawMutex;
//! use embedded_mqtt::{new, Config, State};
//!
//! let mut state = State::<NoopRawMutex, 1024, 1024>::new();
//! let config = Config::builder()
//!     .client_id("my-device".try_into().unwrap())
//!     .build();
//!
//! let (mqtt_stack, mqtt_client) = new(&mut state, config);
//!
//! // Drive mqtt_stack.run(&mut transport) in a background task.
//! // Use mqtt_client.publish(..) and mqtt_client.subscribe(..) from application code.
//! ```
//!
//! # Transport
//!
//! The stack is transport-agnostic. Implement the [`transport::Transport`] trait for your
//! network stack, or use the built-in `embedded-nal` adapter via
//! [`transport::embedded_nal::NalTransport`].
//!
//! # Protocol version
//!
//! Enable exactly one of the `mqttv5` or `mqttv3` feature flags. Enabling both or
//! neither will produce a compile error.

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
mod message;
mod packet;
mod stack;
mod state;
mod topic_filter;

#[cfg(feature = "_test")]
pub mod pubsub;

#[cfg(not(feature = "_test"))]
mod pubsub;

use core::mem::MaybeUninit;

use crate_config::MAX_SUBSCRIBERS;
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};

pub use broker::{Broker, DomainBroker, IpBroker};
pub use client::{MqttClient, Subscription};
pub use config::Config;
pub use message::Message;
pub use pubsub::{
    BufferProvider, FrameGrantR, FrameGrantW, FramePublisher, FrameSubscriber, PubSubChannel,
    SliceBufferProvider,
};
pub use stack::MqttStack;
use state::Shared;

pub use encoding::*;
pub use error::Error;

#[cfg(feature = "mqttv5")]
pub use encoding::{Properties, Property, RetainHandling};

/// Backing storage for the MQTT stack and client, parameterized by TX/RX buffer sizes.
pub struct State<M: RawMutex, const TX: usize, const RX: usize> {
    tx: [u8; TX],
    rx: [u8; RX],
    inner: MaybeUninit<StateInner<'static, M>>,
}

impl<M: RawMutex, const TX: usize, const RX: usize> Default for State<M, TX, RX> {
    fn default() -> Self {
        Self::new()
    }
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

struct StateInner<'a, M: RawMutex> {
    pub(crate) tx: PubSubChannel<M, SliceBufferProvider<'a>, 1>,
    pub(crate) rx_pubsub: PubSubChannel<M, SliceBufferProvider<'a>, MAX_SUBSCRIBERS>,
    pub(crate) shared: Mutex<M, Shared>,
}

/// Creates a new MQTT client and stack.
///
/// This function initializes an MQTT client along with a background "stack" that needs to be
/// executed in a long-running async task. The stack handles the underlying MQTT protocol
/// operations, while the client provides an interface for publishing and subscribing to topics.
///
/// # Parameters
///
/// - `state`: A mutable reference to the state required for the MQTT stack and client. This state
///   must outlive the returned MQTT client and stack.
/// - `config`: The configuration settings for the MQTT client, including client ID,
///   and other MQTT options.
///
/// # Returns
///
/// A tuple containing:
/// - `MqttStack<'d, M>`: The MQTT stack that handles the protocol operations. This stack must be
///   run in a long-running async task.
/// - `MqttClient<'d, M>`: The MQTT client that provides an interface for publishing and subscribing
///   to topics.
///
/// # Safety
///
/// This function involves creating a self-referential struct. The safety guarantees are:
/// - The struct cannot move while the `'d` borrow is active.
/// - When the borrow ends, the dangling references inside the `MaybeUninit` will never be used again.
///
/// # Example
///
/// ```ignore
/// use embedded_mqtt::{new, Config, State};
/// use embassy_sync::blocking_mutex::raw::NoopRawMutex;
///
/// let mut state = State::<NoopRawMutex, 1024, 1024>::new();
/// let config = Config::default();
/// let (mqtt_stack, mqtt_client) = new(&mut state, config);
///
/// // Run the MQTT stack in a long-running async task
/// embassy_executor::spawn(mqtt_task(mqtt_stack)).unwrap();
///
/// // Use the MQTT client to publish and subscribe to topics
/// mqtt_client.subscribe(..).await.unwrap();
/// mqtt_client.publish(..).await.unwrap();
/// ```
pub fn new<'d, M: RawMutex, const TX: usize, const RX: usize>(
    state: &'d mut State<M, TX, RX>,
    config: Config<'d>,
) -> (MqttStack<'d, M>, MqttClient<'d, M>) {
    // safety: this is a self-referential struct, however:
    // - it can't move while the `'d` borrow is active.
    // - when the borrow ends, the dangling references inside the MaybeUninit will never be used again.
    let state_uninit: *mut MaybeUninit<StateInner<'d, M>> =
        (&mut state.inner as *mut MaybeUninit<StateInner<'static, M>>).cast();

    let state = unsafe { &mut *state_uninit }.write(StateInner {
        tx: PubSubChannel::<_, _, 1>::new(SliceBufferProvider::new(&mut state.tx[..])),
        rx_pubsub: PubSubChannel::<_, _, { MAX_SUBSCRIBERS }>::new(SliceBufferProvider::new(
            &mut state.rx[..],
        )),
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

pub(crate) struct OnDrop<F: FnOnce()> {
    f: MaybeUninit<F>,
}

impl<F: FnOnce()> OnDrop<F> {
    pub fn new(f: F) -> Self {
        Self {
            f: MaybeUninit::new(f),
        }
    }

    pub fn defuse(self) {
        core::mem::forget(self)
    }
}

impl<F: FnOnce()> Drop for OnDrop<F> {
    fn drop(&mut self) {
        unsafe { self.f.as_ptr().read()() }
    }
}

#[cfg(not(test))]
pub mod crate_config {
    #![allow(unused)]
    include!(concat!(env!("OUT_DIR"), "/config.rs"));
}

#[cfg(test)]
pub mod crate_config {
    #![allow(unused)]
    // BEGIN AUTOGENERATED CONFIG FEATURES
    // Generated by gen_config.py. DO NOT EDIT.
    pub const MAX_CLIENT_ID_LEN: usize = 64;
    pub const MAX_TOPIC_LEN: usize = 128;
    pub const MAX_SUB_TOPICS_PER_MSG: usize = 8;
    pub const MAX_SUBSCRIBERS: usize = 8;
    pub const MAX_INFLIGHT: usize = 8;
    pub const MAX_PUBSUB_MESSAGES: usize = 16;
    // END AUTOGENERATED CONFIG FEATURES
}
