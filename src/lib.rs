#![no_std]

extern crate alloc;

mod options;
mod requests;
mod state;

use state::{MqttState, StateError};

use alloc::{borrow::ToOwned, collections::VecDeque};
use bytes::BytesMut;
use embedded_nal::{Mode, TcpStack};
use heapless::{spsc::Consumer, ArrayLength};
use mqttrs::{decode, encode, Packet, Pid, Suback};

pub use mqttrs::{Connect, Protocol, Publish, QoS, Subscribe, SubscribeTopic, Unsubscribe};
pub use options::MqttOptions;
pub use requests::{PublishRequest, Request, SubscribeRequest};

/// Includes incoming packets from the network and other interesting events happening in the eventloop
#[derive(Debug)]
pub enum Notification {
    /// Incoming publish from the broker
    Publish(Publish),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MqttConnectionStatus {
    Handshake,
    Connected,
    Disconnecting,
    Disconnected,
}

/// Critical errors during eventloop polling
#[derive(Debug)]
pub enum EventError {
    MqttState(StateError),
    Timeout,
    Encoding(mqttrs::Error),
    // Network(network::Error),
    Network,
    StreamDone,
    RequestsDone,
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

pub struct MqttEvent<'a, L, N, I, O>
where
    L: ArrayLength<Request>,
    N: TcpStack,
{
    /// Current state of the connection
    state: MqttState<I, O>,
    /// Options of the current mqtt connection
    options: MqttOptions,
    /// Receive buffer for incomming packets
    rx_buf: BytesMut,
    /// Network socket
    socket: Option<N::TcpSocket>,
    /// Request stream
    requests: Consumer<'a, Request, L>,

    pending_pub: VecDeque<Publish>,
    pending_rel: VecDeque<Pid>,
}

impl<'a, L, N, I, O> MqttEvent<'a, L, N, I, O>
where
    L: ArrayLength<Request>,
    N: TcpStack,
    I: embedded_hal::timer::CountDown,
    O: embedded_hal::timer::CountDown,
    I::Time: From<u32>,
    O::Time: From<u32>,
{
    pub fn new(
        requests: Consumer<'a, Request, L>,
        incoming_timer: I,
        outgoing_timer: O,
        options: MqttOptions,
    ) -> Self {
        MqttEvent {
            state: MqttState::new(incoming_timer, outgoing_timer),
            rx_buf: BytesMut::with_capacity(options.max_packet_size()),
            options,
            socket: None,
            requests,
            pending_pub: VecDeque::new(),
            pending_rel: VecDeque::new(),
        }
    }

    pub fn connect(&mut self, network: &N) -> Result<(), EventError> {
        self.state.await_pingresp = false;

        // connect to the broker
        self.socket = Some(self.network_connect(network)?);
        self.mqtt_connect(network)?;

        // Handle state after reconnect events
        // self.populate_pending();

        Ok(())
    }

    pub fn yield_event(&mut self, network: &N) -> nb::Result<Notification, EventError> {
        self.receive(network)?;

        let o = if let Some(packet) = decode(&mut self.rx_buf).map_err(EventError::Encoding)? {
            // Handle incoming
            self.state.handle_incoming_packet(packet)
        } else if let Some(p) = self.pending_rel.pop_front() {
            // Handle pending PubRec
            self.state
                .handle_outgoing_packet(Packet::Pubrec(p), self.options.keep_alive_ms())
        } else if let Some(p) = self.pending_pub.pop_front() {
            // Handle pending Publish
            #[cfg(feature = "logging")]
            log::warn!("Handle pending Publish!!");
            self.state
                .handle_outgoing_packet(Packet::Publish(p), self.options.keep_alive_ms())
        } else if !self.state.outgoing_pub.len() >= self.options.inflight() && self.requests.ready()
        {
            // Handle requests
            let request = self.requests.dequeue().unwrap();
            self.state
                .handle_outgoing_request(request, self.options.keep_alive_ms())
        } else if self.state.last_outgoing_timer.wait().is_ok() {
            // Handle ping
            self.state
                .handle_outgoing_packet(Packet::Pingreq, self.options.keep_alive_ms())
                .map_err(|e| nb::Error::Other(e.into()))?;
            Ok((None, Some(Packet::Pingreq)))
        } else {
            Ok((None, None))
        };

        let (notification, outpacket) = match o {
            Ok((n, p)) => (n, p),
            Err(e) => {
                return Ok(Notification::Abort(e.into()));
            }
        };

        if let Some(p) = outpacket {
            if let Err(e) = self.send(network, p) {
                return Ok(Notification::Abort(e.into()));
            }
        }

        if let Some(n) = notification {
            return Ok(n);
        }

        return Err(nb::Error::WouldBlock);
    }

    // fn populate_pending(&mut self) {
    //     let mut pending_pub = core::mem::replace(&mut self.state.outgoing_pub, VecDeque::new());
    //     self.pending_pub.append(&mut pending_pub);

    //     let mut pending_rel = core::mem::replace(&mut self.state.outgoing_rel, VecDeque::new());
    //     self.pending_rel.append(&mut pending_rel);
    // }

    fn send(&mut self, network: &N, pkt: Packet) -> Result<(), EventError> {
        let mut buf = BytesMut::with_capacity(self.options.max_packet_size());
        encode(&pkt, &mut buf)?;

        if let Some(ref mut socket) = self.socket {
            nb::block!(network.write(socket, &buf)).map_err(|_| EventError::Timeout)?;

            Ok(())
        } else {
            Err(EventError::Network)
        }
    }

    fn receive(&mut self, network: &N) -> Result<(), EventError> {
        if let Some(ref mut socket) = self.socket {
            // TODO: It seems silly to create an additional buffer here, only to copy more
            let mut buf = [0u8; 256];
            match network.read(socket, &mut buf) {
                Ok(size) => {
                    if size > 0 {
                        self.rx_buf.extend_from_slice(&buf[0..size]);
                    }
                    Ok(())
                }
                Err(nb::Error::WouldBlock) => Ok(()),
                _ => Err(EventError::Network),
            }
        } else {
            return Err(EventError::Network);
        }
    }

    fn network_connect(&mut self, network: &N) -> Result<N::TcpSocket, EventError> {
        let soc = network
            .open(Mode::Timeout(50))
            .map_err(|_e| EventError::Network)?;

        Ok(network
            .connect(soc, self.options.broker_address().into())
            .map_err(|_e| EventError::Network)?)
    }

    fn mqtt_connect(&mut self, network: &N) -> Result<(), EventError> {
        let (username, password) = if let Some((username, password)) = self.options.credentials() {
            (Some(username), Some(password.as_bytes().to_owned()))
        } else {
            (None, None)
        };

        let connect = Connect {
            protocol: Protocol::MQTT311,
            keep_alive: (self.options.keep_alive_ms() / 1000) as u16,
            client_id: self.options.client_id(),
            clean_session: self.options.clean_session(),
            last_will: self.options.last_will(),
            username,
            password,
        };

        // mqtt connection with timeout
        self.send(network, Packet::Connect(connect))?;
        self.state.handle_outgoing_connect(5000)?;

        loop {
            if self.state.last_outgoing_timer.wait().is_ok() {
                return Err(EventError::Timeout);
            }

            self.receive(network)?;

            match decode(&mut self.rx_buf).map_err(|e| EventError::Encoding(e))? {
                Some(packet) => {
                    self.state.handle_incoming_connack(packet)?;
                    return Ok(());
                }
                None => {}
            };
        }
    }
}
