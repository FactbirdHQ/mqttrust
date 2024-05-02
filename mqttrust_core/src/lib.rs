#![cfg_attr(not(test), no_std)]

#[cfg(feature = "std")]
extern crate std;

// This mod MUST go first, so that the others see its macros.
pub(crate) mod fmt;

mod client;
mod eventloop;
mod options;
mod packet;
mod state;

pub use bbqueue;

pub use client::Client;
use core::convert::TryFrom;
pub use eventloop::EventLoop;
use heapless::{String, Vec};
use mqttrust::encoding::v4::Connack;
pub use mqttrust::encoding::v4::{Pid, Publish, QoS, QosPid, Suback};
pub use mqttrust::*;
pub use options::{Broker, MqttOptions};
use state::StateError;

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "defmt-impl", derive(defmt::Format))]
pub struct PublishNotification {
    pub dup: bool,
    pub qospid: QoS,
    pub retain: bool,
    pub topic_name: String<256>,
    pub payload: Vec<u8, 4096>,
}

/// Includes incoming packets from the network and other interesting events
/// happening in the eventloop
#[derive(Debug, PartialEq)]
// #[cfg_attr(feature = "defmt-impl", derive(defmt::Format))]
pub enum Notification {
    /// Incoming connection acknowledge
    ConnAck(Connack),
    /// Incoming publish from the broker
    #[cfg(not(feature = "std"))]
    Publish(heapless::pool::singleton::Box<state::BoxedPublish, heapless::pool::Init>),
    #[cfg(feature = "std")]
    Publish(std::boxed::Box<PublishNotification>),
    /// Incoming puback from the broker
    Puback(Pid),
    /// Incoming pubrec from the broker
    Pubrec(Pid),
    /// Incoming pubcomp from the broker
    Pubcomp(Pid),
    // TODO:
    // Suback(Suback),
    /// Incoming suback from the broker
    Suback(Pid),
    /// Incoming unsuback from the broker
    Unsuback(Pid),
    // Eventloop error
    Abort(EventError),
}

impl<'a> TryFrom<Publish<'a>> for PublishNotification {
    type Error = StateError;

    fn try_from(p: Publish<'a>) -> Result<Self, Self::Error> {
        Ok(PublishNotification {
            dup: p.dup,
            qospid: p.qos,
            retain: p.retain,
            topic_name: String::from(p.topic_name),
            payload: Vec::from_slice(p.payload).map_err(|_| {
                error!("Failed to convert payload to notification!");
                StateError::PayloadEncoding
            })?,
        })
    }
}

/// Critical errors during eventloop polling
#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "defmt-impl", derive(defmt::Format))]
pub enum EventError {
    MqttState(StateError),
    Timeout,
    Encoding(mqttrust::encoding::v4::Error),
    Network(NetworkError),
    BufferSize,
    Clock,
    RequestsNotAvailable,
}

#[derive(Debug, PartialEq)]
#[cfg_attr(feature = "defmt-impl", derive(defmt::Format))]
pub enum NetworkError {
    Read,
    Write,
    NoSocket,
    SocketOpen,
    SocketConnect,
    SocketClosed,
    DnsLookupFailed,
}

impl From<mqttrust::encoding::v4::Error> for EventError {
    fn from(e: mqttrust::encoding::v4::Error) -> Self {
        EventError::Encoding(e)
    }
}

impl From<StateError> for EventError {
    fn from(e: StateError) -> Self {
        EventError::MqttState(e)
    }
}
