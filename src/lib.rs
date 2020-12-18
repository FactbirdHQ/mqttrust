#![no_std]

mod client;
mod options;
mod requests;
mod state;

use core::convert::TryFrom;
use core::ops::RangeTo;
use embedded_nal::{AddrType, Dns, TcpClient};
use heapless::{consts, spsc, ArrayLength, String, Vec};
use mqttrs::{decode_slice, encode_slice, Pid, Suback};
use no_std_net::SocketAddr;
use state::{MqttState, StateError};

pub use client::{Mqtt, MqttClient, MqttClientError};
pub use mqttrs::{
    Connect, Packet, Protocol, Publish, QoS, QosPid, Subscribe, SubscribeReturnCodes,
    SubscribeTopic, Unsubscribe,
};
pub use options::{Broker, MqttOptions};
pub use requests::{PublishPayload, PublishRequest, Request, SubscribeRequest, UnsubscribeRequest};

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
            payload: Vec::from_slice(p.payload).map_err(|_| StateError::PayloadEncoding)?,
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

pub struct MqttEvent<'a, 'b, L, S, O, P>
where
    L: ArrayLength<Request<P>>,
    P: PublishPayload,
{
    /// Current state of the connection
    pub state: MqttState<O, P>,
    /// Options of the current mqtt connection
    pub options: MqttOptions<'b>,
    /// Network socket
    pub socket: Option<S>,
    /// Request stream
    pub requests: spsc::Consumer<'a, Request<P>, L, u8>,
    tx_buf: Vec<u8, consts::U1024>,
    rx_buf: PacketBuffer,
}

impl<'a, 'b, L, S, O, P> MqttEvent<'a, 'b, L, S, O, P>
where
    L: ArrayLength<Request<P>>,
    O: embedded_hal::timer::CountDown,
    O::Time: From<u32>,
    P: PublishPayload + Clone,
{
    pub fn new(
        requests: spsc::Consumer<'a, Request<P>, L, u8>,
        outgoing_timer: O,
        options: MqttOptions<'b>,
    ) -> Self {
        MqttEvent {
            state: MqttState::new(outgoing_timer),
            options,
            socket: None,
            requests,
            tx_buf: Vec::new(),
            rx_buf: PacketBuffer::new(),
        }
    }

    pub fn connect<N: Dns + TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<bool, EventError> {
        // connect to the broker
        self.network_connect(network)?;
        if self.mqtt_connect(network)? {
            return Ok(true);
        }

        Ok(false)
    }

    fn should_handle_request(&mut self) -> bool {
        let qos_space = self.state.outgoing_pub.len() < self.options.inflight();

        let qos_0 = if let Some(Request::Publish(p)) = self.requests.peek() {
            p.qos == QoS::AtMostOnce
        } else {
            false
        };

        if qos_0 {
            true
        } else {
            self.requests.ready() && qos_space
        }
    }

    pub fn yield_event<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<Notification, ()> {
        let packet_buf = &mut [0u8; 1024];
        let o = if self.should_handle_request() {
            // Handle requests
            let request = unsafe { self.requests.dequeue_unchecked() };
            self.state
                .handle_outgoing_request(request, packet_buf)
                .map_err(EventError::from)
        } else if self.state.last_outgoing_timer.try_wait().is_ok() {
            // Handle ping
            self.state
                .handle_outgoing_packet(Packet::Pingreq)
                .map_err(EventError::from)
        } else {
            self.receive_notification(network).map_err(|e| {
                self.rx_buf.init();
                e
            })
        };

        let (notification, outpacket) = match o {
            Ok((n, p)) => (n, p),
            Err(e) => {
                defmt::debug!("Disconnecting from handling error!");
                self.disconnect(network);
                return Ok(Notification::Abort(e));
            }
        };

        if let Some(p) = outpacket {
            if let Err(e) = self.send(network, p) {
                defmt::debug!("Disconnecting from send error!");
                self.disconnect(network);
                return Ok(Notification::Abort(e));
            } else {
                self.state
                    .last_outgoing_timer
                    .try_start(self.options.keep_alive_ms())
                    .ok();
            }
        }

        if let Some(n) = notification {
            Ok(n)
        } else {
            Err(nb::Error::WouldBlock)
        }
    }

    pub fn send<'d, N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
        pkt: Packet<'d>,
    ) -> Result<usize, EventError> {
        match self.socket {
            Some(ref mut socket) => {
                let capacity = self.tx_buf.capacity();
                self.tx_buf.clear();
                self.tx_buf.resize(capacity, 0x00u8).unwrap_or_else(|()| {
                    unreachable!("Input length equals to the current capacity.")
                });
                let size = encode_slice(&pkt, self.tx_buf.as_mut())?;
                nb::block!(network.send(socket, &self.tx_buf[..size])).map_err(|_| {
                    defmt::error!("[send] NetworkError::Write");
                    EventError::Network(NetworkError::Write)
                })
            }
            _ => Err(EventError::Network(NetworkError::NoSocket)),
        }
    }

    pub fn receive_notification<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), EventError> {
        let socket = self
            .socket
            .as_mut()
            .ok_or(EventError::Network(NetworkError::NoSocket))?;
        if let Some(packet) = self.rx_buf.receive(socket, network).or_else(|e| match e {
            nb::Error::WouldBlock => Ok(None),
            _ => Err(EventError::Network(NetworkError::Read)),
        })? {
            self.state
                .handle_incoming_packet(packet)
                .map(|out| {
                    let length = self.rx_buf.packet_length();
                    self.rx_buf.rotate(length);
                    out
                })
                .map_err(EventError::from)
        } else {
            Ok((None, None))
        }
    }

    pub fn receive<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> Result<Option<Packet<'_>>, EventError> {
        let socket = self
            .socket
            .as_mut()
            .ok_or(EventError::Network(NetworkError::NoSocket))?;
        self.rx_buf.receive(socket, network).or_else(|e| match e {
            nb::Error::WouldBlock => Ok(None),
            _ => Err(EventError::Network(NetworkError::Read)),
        })
    }

    fn lookup_host<N: Dns + TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> Result<(heapless::String<heapless::consts::U256>, SocketAddr), EventError> {
        match self.options.broker() {
            (Broker::Hostname(h), p) => {
                let socket_addr = SocketAddr::new(
                    network.gethostbyname(h, AddrType::IPv4).map_err(|_e| {
                        defmt::info!("Failed to resolve IP!");
                        EventError::Network(NetworkError::DnsLookupFailed)
                    })?,
                    p,
                );
                Ok((heapless::String::from(h), socket_addr))
            }
            (Broker::IpAddr(ip), p) => {
                let socket_addr = SocketAddr::new(ip, p);
                let domain = network.gethostbyaddr(ip).map_err(|_e| {
                    defmt::info!("Failed to resolve hostname!");
                    EventError::Network(NetworkError::DnsLookupFailed)
                })?;

                Ok((domain, socket_addr))
            }
        }
    }

    fn network_connect<N: Dns + TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> Result<(), EventError> {
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

        self.state.connection_status = state::MqttConnectionStatus::Disconnected;

        self.socket = network
            .socket()
            .map_err(|_e| EventError::Network(NetworkError::SocketOpen))?
            .into();

        match self.lookup_host(network) {
            Ok((_hostname, socket_addr)) => {
                Some(
                    network
                        .connect(
                            self.socket.as_mut().unwrap_or_else(|| unreachable!()),
                            socket_addr,
                        )
                        .map_err(|_e| EventError::Network(NetworkError::SocketConnect))?,
                );

                // if let Some(root_ca) = self.options.ca() {
                //     // Add root CA
                // };

                // if let Some((certificate, private_key)) = self.options.client_auth() {
                //     // Enable SSL for self.socket, with broker (hostname)
                // };

                defmt::debug!("Network connected!");

                Ok(())
            }
            Err(e) => {
                // Make sure to cleanup socket, in case we fail DNS lookup for some reason
                self.disconnect(network);
                Err(e)
            }
        }
    }

    pub fn disconnect<N: TcpClient<TcpSocket = S>>(&mut self, network: &N) {
        self.state.connection_status = state::MqttConnectionStatus::Disconnected;
        if let Some(socket) = self.socket.take() {
            network.close(socket).ok();
        }
    }

    fn mqtt_connect<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<bool, EventError> {
        match self.state.connection_status {
            state::MqttConnectionStatus::Connected => Ok(false),
            state::MqttConnectionStatus::Disconnected => {
                defmt::info!("MQTT connecting..");
                self.state.await_pingresp = false;
                self.state.outgoing_pub.clear();

                let (username, password) = self.options.credentials();

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

                        self.state.last_outgoing_timer.try_start(50000).ok();
                    }
                    Err(e) => {
                        defmt::debug!("Disconnecting from send error!");
                        self.disconnect(network);
                        return Err(nb::Error::Other(e));
                    }
                }

                Err(nb::Error::WouldBlock)
            }
            state::MqttConnectionStatus::Handshake => {
                if self.state.last_outgoing_timer.try_wait().is_ok() {
                    defmt::debug!("Disconnecting from handshake timeout!");
                    self.disconnect(network);
                    return Err(nb::Error::Other(EventError::Timeout));
                }

                match self.receive(network) {
                    Ok(Some(Packet::Connack(connack))) => {
                        self.state
                            .handle_incoming_connack(connack)
                            .map_err(|e| nb::Error::Other(e.into()))?;

                        defmt::debug!("MQTT connected!");

                        Ok(true)
                    }
                    Ok(_) => Err(nb::Error::WouldBlock),
                    Err(e) => Err(nb::Error::Other(e)),
                }
            }
        }
    }
}

// A placeholder that keeps a buffer and constructs a packet incrementally.
// Given that underlying TcpClient throws WouldBlock in a non-blocking manner,
// its packet construction won't block either.
struct PacketBuffer {
    range: RangeTo<usize>,
    buffer: Vec<u8, consts::U1024>,
}

impl PacketBuffer {
    fn new() -> Self {
        let range = ..0;
        let buffer = Vec::new();
        let mut buf = Self { range, buffer };
        buf.init();
        buf
    }

    // Fills the buffer with all 0s
    fn init(&mut self) {
        self.range.end = 0;
        let capacity = self.buffer.capacity();
        self.buffer.clear();
        self.buffer
            .resize(capacity, 0x00u8)
            .unwrap_or_else(|()| unreachable!("Input length equals to the current capacity."));
    }

    // Returns a remaining fresh part of the buffer.
    fn buffer(&mut self) -> &mut [u8] {
        let range = self.range.end..;
        self.buffer[range].as_mut()
    }

    // Doesn't cover these cases: 1. the 4th byte has its continuation bit set
    // (i.e., the packet is malformed). Nor 2. a successor byte has not yet
    // transferred even if the continuation bit suggests its presence.
    //
    // https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718023
    fn packet_length(&self) -> usize {
        self.buffer
            .iter()
            .skip(1)
            .take(4)
            .scan(true, |continuation, byte| {
                let has_successor = byte & 0x80 != 0x00;
                let length = (byte & 0x7f) as usize;
                if *continuation {
                    *continuation = has_successor;
                    length.into()
                } else {
                    // Short-circuit
                    None
                }
            })
            .enumerate()
            .fold(1, |acc, (i, length)| {
                acc + 1 + length * 0x80_usize.pow(i as u32)
            })
    }

    // Invariant: buffer.capacity() == buffer.len()
    // Assumes length < buffer.len() and length < buffer.range.end
    fn rotate(&mut self, length: usize) {
        self.buffer.rotate_left(length);
        self.range.end -= length;
        self.buffer.truncate(self.buffer.len() - length);
        self.buffer.resize(self.buffer.capacity(), 0).unwrap();
    }

    // Assuming the buffer contains a possibly half-done packet. If a complete
    // packet is found, returns it. Post condition is that range is set to 0
    // when the encoder constructs a full-packet or raises an error.
    fn response(&mut self) -> Result<Option<Packet<'_>>, EventError> {
        let buffer = self.buffer[self.range].as_ref();
        let packet = decode_slice(buffer);
        packet.map_err(EventError::Encoding)
    }

    // If incoming bytes found, the range gets extended covering them.
    fn receive<N, S>(
        &mut self,
        socket: &mut S,
        network: &N,
    ) -> nb::Result<Option<Packet<'_>>, EventError>
    where
        N: TcpClient<TcpSocket = S>,
    {
        let buffer = self.buffer();
        let len = network
            .receive(socket, buffer)
            .map_err(|e| e.map(|_| EventError::Network(NetworkError::Read)))?;
        self.range.end += len;
        self.response().map_err(nb::Error::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_hal::timer::CountDown;
    use heapless::{consts, spsc::Queue, String, Vec};
    use mqttrs::{Connack, ConnectReturnCode};

    #[derive(Debug)]
    struct CdMock {
        time: u32,
    }

    impl CountDown for CdMock {
        type Error = core::convert::Infallible;
        type Time = u32;
        fn try_start<T>(&mut self, count: T) -> Result<(), Self::Error>
        where
            T: Into<Self::Time>,
        {
            self.time = count.into();
            Ok(())
        }
        fn try_wait(&mut self) -> nb::Result<(), Self::Error> {
            Ok(())
        }
    }

    struct MockNetwork {
        pub should_fail_read: bool,
        pub should_fail_write: bool,
    }

    impl Dns for MockNetwork {
        type Error = ();

        fn gethostbyname(
            &self,
            _hostname: &str,
            _addr_type: embedded_nal::AddrType,
        ) -> Result<embedded_nal::IpAddr, Self::Error> {
            unimplemented!()
        }
        fn gethostbyaddr(
            &self,
            _addr: embedded_nal::IpAddr,
        ) -> Result<heapless::String<consts::U256>, Self::Error> {
            unimplemented!()
        }
    }

    impl TcpClient for MockNetwork {
        type TcpSocket = ();
        type Error = ();

        fn socket(&self) -> Result<Self::TcpSocket, Self::Error> {
            Ok(())
        }

        fn connect(
            &self,
            _socket: &mut Self::TcpSocket,
            _remote: embedded_nal::SocketAddr,
        ) -> nb::Result<(), Self::Error> {
            Ok(())
        }

        fn is_connected(&self, _socket: &Self::TcpSocket) -> Result<bool, Self::Error> {
            Ok(true)
        }

        fn send(
            &self,
            _socket: &mut Self::TcpSocket,
            buffer: &[u8],
        ) -> nb::Result<usize, Self::Error> {
            if self.should_fail_write {
                Err(nb::Error::Other(()))
            } else {
                Ok(buffer.len())
            }
        }

        fn receive(
            &self,
            _socket: &mut Self::TcpSocket,
            buffer: &mut [u8],
        ) -> nb::Result<usize, Self::Error> {
            if self.should_fail_read {
                Err(nb::Error::Other(()))
            } else {
                let connack = Packet::Connack(Connack {
                    session_present: false,
                    code: ConnectReturnCode::Accepted,
                });
                let size = encode_slice(&connack, buffer).unwrap();
                Ok(size)
            }
        }

        fn close(&self, _socket: Self::TcpSocket) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    #[ignore]
    fn retry_behaviour() {
        static mut Q: Queue<Request<Vec<u8, consts::U10>>, consts::U5, u8> =
            Queue(heapless::i::Queue::u8());

        let network = MockNetwork {
            should_fail_read: false,
            should_fail_write: false,
        };

        let (_p, c) = unsafe { Q.split() };
        let mut event = MqttEvent::<_, (), _, _>::new(
            c,
            CdMock { time: 0 },
            MqttOptions::new("client", Broker::Hostname(""), 8883),
        );

        event
            .state
            .outgoing_pub
            .insert(
                2,
                PublishRequest {
                    dup: false,
                    qos: QoS::AtLeastOnce,
                    retain: false,
                    topic_name: String::from("some/topic/name2"),
                    payload: Vec::new(),
                },
            )
            .unwrap();

        event
            .state
            .outgoing_pub
            .insert(
                3,
                PublishRequest {
                    dup: false,
                    qos: QoS::AtLeastOnce,
                    retain: false,
                    topic_name: String::from("some/topic/name3"),
                    payload: Vec::new(),
                },
            )
            .unwrap();

        event
            .state
            .outgoing_pub
            .insert(
                4,
                PublishRequest {
                    dup: false,
                    qos: QoS::AtLeastOnce,
                    retain: false,
                    topic_name: String::from("some/topic/name4"),
                    payload: Vec::new(),
                },
            )
            .unwrap();

        event.state.connection_status = state::MqttConnectionStatus::Handshake;
        event.socket = Some(());

        event.connect(&network).unwrap();

        // assert_eq!(event.pending_pub.len(), 3);

        // let mut key_iter = event.pending_pub.keys();
        // assert_eq!(key_iter.next(), Some(&2));
        // assert_eq!(key_iter.next(), Some(&3));
        // assert_eq!(key_iter.next(), Some(&4));
    }
}
