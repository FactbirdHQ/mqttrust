#![cfg_attr(not(test), no_std)]

mod client;
mod eventloop;
mod options;
mod state;

pub use client::{Client, MqttClientError};
use core::convert::TryFrom;
use embedded_time::clock;
pub use eventloop::EventLoop;
use heapless::{consts, String, Vec};
use mqttrs::Pid;
pub use mqttrs::{
    Connect, Packet, Protocol, Publish, QoS, QosPid, Suback, Subscribe, SubscribeReturnCodes,
    SubscribeTopic, Unsubscribe,
};
pub use mqttrust::{PublishPayload, PublishRequest, Request, SubscribeRequest, UnsubscribeRequest};
pub use options::{Broker, MqttOptions};
use state::StateError;

#[derive(Debug, PartialEq)]
pub struct PublishNotification {
    pub dup: bool,
    pub qospid: QosPid,
    pub retain: bool,
    pub topic_name: String<consts::U256>,
    pub payload: Vec<u8, consts::U2048>,
}

/// Includes incoming packets from the network and other interesting events
/// happening in the eventloop
#[derive(Debug, PartialEq)]
pub enum Notification {
    /// Incoming connection acknowledge
    ConnAck,
    /// Incoming publish from the broker
    Publish(PublishNotification),
    /// Incoming puback from the broker
    Puback(Pid),
    /// Incoming pubrec from the broker
    Pubrec(Pid),
    /// Incoming pubcomp from the broker
    Pubcomp(Pid),
    /// Incoming suback from the broker
    Suback(Suback),
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
            qospid: p.qospid,
            retain: p.retain,
            topic_name: String::from(p.topic_name),
            payload: Vec::from_slice(p.payload).map_err(|_| {
                defmt::error!("Failed to convert payload to notification!");
                StateError::PayloadEncoding
            })?,
        })
    }
}

/// Critical errors during eventloop polling
#[derive(Debug, PartialEq)]
pub enum EventError {
    MqttState(StateError),
    Timeout,
    Encoding(mqttrs::Error),
    Network(NetworkError),
    BufferSize,
    Clock,
}

#[derive(Debug, PartialEq, defmt::Format)]
pub enum NetworkError {
    Read,
    Write,
    NoSocket,
    SocketOpen,
    SocketConnect,
    SocketClosed,
    DnsLookupFailed,
}

impl From<mqttrs::Error> for EventError {
    fn from(e: mqttrs::Error) -> Self {
        EventError::Encoding(e)
    }
}

impl From<StateError> for EventError {
    fn from(e: StateError) -> Self {
        EventError::MqttState(e)
    }
}

impl From<clock::Error> for EventError {
    fn from(_e: clock::Error) -> Self {
        EventError::Clock
    }
}

#[cfg(test)]
mod tests {
    //! This module is required in order to satisfy the requirements of defmt, while running tests.
    //! Note that this will cause all log `defmt::` log statements to be thrown away.

    use core::ptr::NonNull;

    #[defmt::global_logger]
    struct Logger;
    impl defmt::Write for Logger {
        fn write(&mut self, _bytes: &[u8]) {}
    }

    unsafe impl defmt::Logger for Logger {
        fn acquire() -> Option<NonNull<dyn defmt::Write>> {
            Some(NonNull::from(&Logger as &dyn defmt::Write))
        }

        unsafe fn release(_: NonNull<dyn defmt::Write>) {}
    }

    defmt::timestamp!("");

    #[export_name = "_defmt_panic"]
    fn panic() -> ! {
        panic!()
    }
}
