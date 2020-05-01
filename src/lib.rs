#![no_std]
#![feature(maybe_uninit_slice_assume_init)]

extern crate alloc;

mod client;
mod options;
mod requests;
mod state;

use state::{MqttState, StateError};

use alloc::borrow::ToOwned;
use bytes::{BufMut, BytesMut};
use embedded_nal::{Mode, TcpStack};
use heapless::{spsc::Consumer, ArrayLength};
use mqttrs::{decode, encode, Packet, Pid, Suback};

pub use client::{Mqtt, MqttClient, MqttClientError};
pub use mqttrs::{Connect, Protocol, Publish, QoS, QosPid, Subscribe, SubscribeTopic, Unsubscribe};
pub use options::MqttOptions;
pub use requests::{PublishRequest, Request, SubscribeRequest, UnsubscribeRequest};

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
    Network(NetworkError),
}

#[derive(Debug)]
pub enum NetworkError {
    Read,
    Write,
    NoSocket,
    SocketOpen,
    SocketConnect,
    SocketClosed,
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

pub struct MqttEvent<'a, 'b, L, N, O>
where
    L: ArrayLength<Request>,
    N: TcpStack,
{
    /// Current state of the connection
    state: MqttState<O>,
    /// Options of the current mqtt connection
    options: MqttOptions<'b>,
    /// Receive buffer for incomming packets
    rx_buf: BytesMut,
    /// Network socket
    socket: Option<N::TcpSocket>,
    /// Request stream
    requests: Consumer<'a, Request, L>,
    // pending_pub: VecDeque<Publish>,
    // pending_rel: VecDeque<Pid>,
}

impl<'a, 'b, L, N, O> MqttEvent<'a, 'b, L, N, O>
where
    L: ArrayLength<Request>,
    N: TcpStack,
    O: embedded_hal::timer::CountDown,
    O::Time: From<u32>,
{
    pub fn new(
        requests: Consumer<'a, Request, L>,
        outgoing_timer: O,
        options: MqttOptions<'b>,
    ) -> Self {
        MqttEvent {
            state: MqttState::new(outgoing_timer),
            rx_buf: BytesMut::with_capacity(options.max_packet_size()),
            options,
            socket: None,
            requests,
            // pending_pub: VecDeque::new(),
            // pending_rel: VecDeque::new(),
        }
    }

    pub fn connect(&mut self, network: &N) -> nb::Result<(), EventError> {
        // connect to the broker
        self.network_connect(network)?;
        self.mqtt_connect(network)?;

        // Handle state after reconnect events
        // self.populate_pending();

        Ok(())
    }

    pub fn yield_event(&mut self, network: &N) -> nb::Result<Notification, ()> {
        let incoming = match self.receive(network) {
            Ok(p) => p,
            Err(EventError::Encoding(e)) => return Ok(Notification::Abort(e.into())),
            Err(e) => {
                self.disconnect(network);
                return Ok(Notification::Abort(e));
            }
        };

        let o = if let Some(packet) = incoming {
            // Handle incoming
            self.state.handle_incoming_packet(packet)
        // } else if let Some(p) = self.pending_rel.pop_front() {
        //     // Handle pending PubRec
        //     self.state
        //         .handle_outgoing_packet(Packet::Pubrec(p))
        // } else if let Some(p) = self.pending_pub.pop_front() {
        //     // Handle pending Publish
        //     self.state
        //         .handle_outgoing_packet(Packet::Publish(p))
        } else if self.state.outgoing_pub.len() < self.options.inflight() && self.requests.ready() {
            // Handle requests
            let request = unsafe { self.requests.dequeue_unchecked() };
            self.state.handle_outgoing_request(request)
        } else if self.state.last_outgoing_timer.wait().is_ok() {
            // Handle ping
            self.state.handle_outgoing_packet(Packet::Pingreq)
        } else {
            Ok((None, None))
        };

        let (notification, outpacket) = match o {
            Ok((n, p)) => (n, p),
            Err(e) => {
                self.disconnect(network);
                return Ok(Notification::Abort(e.into()));
            }
        };

        if let Some(p) = outpacket {
            if let Err(e) = self.send(network, p) {
                self.disconnect(network);
                return Ok(Notification::Abort(e));
            } else {
                self.state
                    .last_outgoing_timer
                    .start(self.options.keep_alive_ms());
            }
        }

        if let Some(n) = notification {
            Ok(n)
        } else {
            Err(nb::Error::WouldBlock)
        }
    }

    // fn populate_pending(&mut self) {
    //     let mut pending_pub = core::mem::replace(&mut self.state.outgoing_pub, VecDeque::new());
    //     self.pending_pub.append(&mut pending_pub);

    //     let mut pending_rel = core::mem::replace(&mut self.state.outgoing_rel, VecDeque::new());
    //     self.pending_rel.append(&mut pending_rel);
    // }

    fn send(&mut self, network: &N, pkt: Packet) -> Result<usize, EventError> {
        match self.socket {
            Some(ref mut socket) => {
                let mut tx_buf: [u8; 2048] = [0; 2048];
                let size = encode(&pkt, &mut &mut tx_buf[..])?;
                nb::block!(network.write(socket, &tx_buf[..size]))
                    .map_err(|_| EventError::Network(NetworkError::Write))
            }
            _ => Err(EventError::Network(NetworkError::NoSocket)),
        }
    }

    fn receive(&mut self, network: &N) -> Result<Option<Packet>, EventError> {
        match self.socket {
            Some(ref mut socket) => {
                match network.read(socket, unsafe {
                    core::mem::MaybeUninit::slice_get_mut(self.rx_buf.bytes_mut())
                }) {
                    Ok(size) => {
                        // Should always be safe, as we are only writing to "remaining" bytes
                        unsafe { self.rx_buf.advance_mut(size) };
                        decode(&mut self.rx_buf).map_err(EventError::Encoding)
                    }
                    Err(nb::Error::WouldBlock) => Ok(None),
                    _ => Err(EventError::Network(NetworkError::Read)),
                }
            }
            _ => Err(EventError::Network(NetworkError::NoSocket)),
        }
    }

    fn network_connect(&mut self, network: &N) -> Result<(), EventError> {
        if let Some(socket) = &self.socket {
            match network.is_connected(socket) {
                Ok(true) => return Ok(()),
                Err(_e) => {
                    self.socket = None;
                    return Err(EventError::Network(NetworkError::SocketClosed));
                }
                Ok(false) => {}
            }
        };

        let socket = network
            .open(Mode::Timeout(50))
            .map_err(|_e| EventError::Network(NetworkError::SocketOpen))?;

        self.socket = Some(
            network
                .connect(socket, self.options.broker_address().into())
                .map_err(|_e| EventError::Network(NetworkError::SocketConnect))?,
        );

        // if let Some(root_ca) = self.options.ca() {
        //     // Add root CA
        // };

        // if let Some((certificate, private_key)) = self.options.client_auth() {
        //     // Enable SSL for self.socket, with broker (self.options.broker_address().ip())
        // };

        #[cfg(feature = "logging")]
        log::debug!("Network connected!");

        Ok(())
    }

    fn disconnect(&mut self, network: &N) {
        self.state.connection_status = state::MqttConnectionStatus::Disconnected;
        if let Some(socket) = &self.socket {
            let connected = network.is_connected(socket);
            if connected.is_err() || !connected.unwrap() {
                self.socket = None;
            }
        }
    }

    fn mqtt_connect(&mut self, network: &N) -> nb::Result<(), EventError> {
        match self.state.connection_status {
            state::MqttConnectionStatus::Connected => Ok(()),
            state::MqttConnectionStatus::Disconnected => {
                #[cfg(feature = "logging")]
                log::info!("MQTT connecting..");
                self.state.await_pingresp = false;
                self.rx_buf.clear();

                let (username, password) =
                    if let Some((username, password)) = self.options.credentials() {
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
                match self.send(network, connect.into()) {
                    Ok(_) => {
                        self.state
                            .handle_outgoing_connect()
                            .map_err(|e| nb::Error::Other(e.into()))?;

                        self.state
                            .last_outgoing_timer
                            .start(self.options.keep_alive_ms());
                    }
                    Err(e) => {
                        self.disconnect(network);
                        return Err(nb::Error::Other(e));
                    }
                }

                Err(nb::Error::WouldBlock)
            }
            state::MqttConnectionStatus::Handshake => {
                if self.state.last_outgoing_timer.wait().is_ok() {
                    self.disconnect(network);
                    return Err(nb::Error::Other(EventError::Timeout));
                }

                match self.receive(network)? {
                    Some(packet) => {
                        self.state
                            .handle_incoming_connack(packet)
                            .map_err(|e| nb::Error::Other(e.into()))?;

                        #[cfg(feature = "logging")]
                        log::debug!("MQTT connected!");

                        Ok(())
                    }
                    None => Err(nb::Error::WouldBlock),
                }
            }
            state::MqttConnectionStatus::Disconnecting => Ok(()),
        }
    }
}
