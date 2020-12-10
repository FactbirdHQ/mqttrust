#![no_std]

mod client;
mod options;
mod requests;
mod state;

use no_std_net::SocketAddr;
use state::{MqttState, StateError};

use core::convert::TryFrom;

use embedded_nal::{AddrType, Dns, Mode, TcpStack};
use heapless::{consts, spsc, ArrayLength, String, Vec};
use mqttrs::{decode_slice, encode_slice, Pid, Suback};

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
    // Outgoing QoS 1, 2 publishes which aren't acked yet
    // pending_pub: FnvIndexMap<u16, PublishRequest<P>, consts::U3>,
    // Packet ids of released QoS 2 publishes
    // pending_rel: FnvIndexSet<u16, consts::U4>,
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
            // pending_pub: IndexMap::new(),
            // pending_rel: IndexSet::new(),
        }
    }

    pub fn connect<N: Dns + TcpStack<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<bool, EventError> {
        // connect to the broker
        self.network_connect(network)?;
        if self.mqtt_connect(network)? {
            // Handle state after reconnect events
            // self.populate_pending();
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

    pub fn yield_event<N: TcpStack<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<Notification, ()> {
        let packet_buf = &mut [0u8; 1024];

        let o = if self.should_handle_request() {
            // Handle requests
            let request = unsafe { self.requests.dequeue_unchecked() };
            self.state.handle_outgoing_request(request, packet_buf)
        } else if let Some(packet) = match self.receive(network, packet_buf) {
            Ok(p) => p,
            Err(EventError::Encoding(e)) => {
                defmt::debug!("Encoding error!");
                return Ok(Notification::Abort(e.into()));
            }
            Err(e) => {
                defmt::debug!("Disconnecting from receive error!");
                self.disconnect(network);
                return Ok(Notification::Abort(e));
            }
        } {
            // Handle incoming
            self.state.handle_incoming_packet(packet)
        // } else if let Some(p) = self.get_pending_rel() {
        //     // Handle pending PubRec
        //     self.state.handle_outgoing_packet(Packet::Pubrec(p))
        // } else if let Some(publish) = self.get_pending_pub() {
        //     // Handle pending Publish
        //     self.state
        //         .handle_outgoing_request(publish.into(), packet_buf)
        } else if self.state.last_outgoing_timer.try_wait().is_ok() {
            // Handle ping
            self.state.handle_outgoing_packet(Packet::Pingreq)
        } else {
            Ok((None, None))
        };

        let (notification, outpacket) = match o {
            Ok((n, p)) => (n, p),
            Err(e) => {
                defmt::debug!("Disconnecting from handling error!");
                self.disconnect(network);
                return Ok(Notification::Abort(e.into()));
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

    // pub fn get_pending_rel(&mut self) -> Option<Pid> {
    //     let p = match self.pending_rel.iter().next() {
    //         Some(p) => *p,
    //         None => return None,
    //     };
    //     self.pending_rel.remove(&p);
    //     Pid::try_from(p).ok()
    // }

    // pub fn get_pending_pub(&mut self) -> Option<PublishRequest<P>> {
    //     let pid = match self.pending_pub.keys().next() {
    //         Some(p) => *p,
    //         None => return None,
    //     };
    //     self.pending_pub.remove(&pid)
    // }

    // fn populate_pending(&mut self) {
    //     let pending_pub = core::mem::replace(&mut self.state.outgoing_pub, IndexMap::new());

    //     defmt::info!("Populating pending publish: {:?}", pending_pub.len());

    //     self.pending_pub
    //         .extend(pending_pub.iter().map(|(key, value)| (*key, value.clone())));

    //     let pending_rel = core::mem::replace(&mut self.state.outgoing_rel, IndexSet::new());

    //     defmt::info!("populating pending rel: {:?}", pending_rel.len());

    //     self.pending_rel.extend(pending_rel.iter());
    // }

    pub fn send<'d, N: TcpStack<TcpSocket = S>>(
        &mut self,
        network: &N,
        pkt: Packet<'d>,
    ) -> Result<usize, EventError> {
        match self.socket {
            Some(ref mut socket) => {
                let mut tx_buf: [u8; 1024] = [0; 1024];
                let size = encode_slice(&pkt, &mut tx_buf)?;
                nb::block!(network.write(socket, &tx_buf[..size])).map_err(|_| {
                    defmt::error!("[send] NetworkError::Write");
                    EventError::Network(NetworkError::Write)
                })
            }
            _ => Err(EventError::Network(NetworkError::NoSocket)),
        }
    }

    pub fn receive<'d, N: TcpStack<TcpSocket = S>>(
        &mut self,
        network: &N,
        packet_buf: &'d mut [u8],
    ) -> Result<Option<Packet<'d>>, EventError> {
        match self.socket {
            Some(ref mut socket) => {
                match network.read_with(socket, |a, b| parse_header(a, b, packet_buf)) {
                    Ok(0) | Err(nb::Error::WouldBlock) => Ok(None),
                    Ok(size) => {
                        let p = decode_slice(&packet_buf[..size]).map_err(EventError::Encoding);

                        // if let Ok(Some(Packet::Puback(pid))) = p {
                        //     defmt::info!("Got Puback! {:?}", pid.get());
                        // }
                        p
                    }
                    _ => Err(EventError::Network(NetworkError::Read)),
                }
            }
            _ => Err(EventError::Network(NetworkError::NoSocket)),
        }
    }

    fn lookup_host<N: Dns + TcpStack<TcpSocket = S>>(
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

    fn network_connect<N: Dns + TcpStack<TcpSocket = S>>(
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

        let socket = network
            .open(Mode::Blocking)
            .map_err(|_e| EventError::Network(NetworkError::SocketOpen))?;

        match self.lookup_host(network) {
            Ok((_hostname, socket_addr)) => {
                self.socket = Some(
                    network
                        .connect(socket, socket_addr)
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
                network
                    .close(socket)
                    .map_err(|_e| EventError::Network(NetworkError::SocketClosed))?;
                Err(e)
            }
        }
    }

    pub fn disconnect<N: TcpStack<TcpSocket = S>>(&mut self, network: &N) {
        self.state.connection_status = state::MqttConnectionStatus::Disconnected;
        if let Some(socket) = self.socket.take() {
            network.close(socket).ok();
        }
    }

    fn mqtt_connect<N: TcpStack<TcpSocket = S>>(
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

                let packet_buf = &mut [0u8; 4];
                match self.receive(network, packet_buf) {
                    Ok(Some(packet)) => {
                        self.state
                            .handle_incoming_connack(packet)
                            .map_err(|e| nb::Error::Other(e.into()))?;

                        defmt::debug!("MQTT connected!");

                        Ok(true)
                    }
                    Ok(None) => Err(nb::Error::WouldBlock),
                    Err(e) => Err(nb::Error::Other(e)),
                }
            }
        }
    }
}

fn valid_header(hd: u8) -> bool {
    match hd >> 4 {
        3 => true,
        6 | 8 | 10 => hd & 0x0F == 0x02,
        1..=2 | 4..=5 | 7 | 9 | 11..=14 => hd.trailing_zeros() >= 4,
        _ => false,
    }
}

fn parse_header(a: &[u8], b: Option<&[u8]>, output: &mut [u8]) -> usize {
    if a.is_empty() || !valid_header(a[0]) {
        return 0;
    }

    let mut len: usize = 0;
    for pos in 0..=3 {
        match b {
            Some(b) if a.len() + b.len() > pos + 1 => {
                // a contains atleast partial header, a + b contains rest of packet
                let byte = if a.len() > pos + 1 {
                    a[pos + 1]
                } else {
                    b[pos + 1 - a.len()]
                };
                len += (byte as usize & 0x7F) << (pos * 7);
                if (byte & 0x80) == 0 {
                    // Continuation bit == 0, length is parsed
                    let packet_len = 2 + pos + len;
                    if a.len() + b.len() < packet_len {
                        // a+b does not contain the full payload
                        return 0;
                    } else {
                        if output.len() < packet_len {
                            defmt::error!(
                                "Output buffer too small! {:?} < {:?}",
                                output.len(),
                                packet_len
                            );
                            return 0;
                        }

                        let a_copy = core::cmp::min(a.len(), packet_len);
                        output[..a_copy].copy_from_slice(&a[..a_copy]);
                        if packet_len > a_copy {
                            output[a_copy..packet_len].copy_from_slice(&b[..packet_len - a_copy]);
                        }
                        return packet_len;
                    }
                }
            }
            None if a.len() > pos + 1 => {
                // a contains the full packet
                let byte = a[pos + 1];
                len += (byte as usize & 0x7F) << (pos * 7);
                if (byte & 0x80) == 0 {
                    // Continuation bit == 0, length is parsed
                    let packet_len = 2 + pos + len;
                    if a.len() < packet_len {
                        // a does not contain the full payload
                        return 0;
                    }

                    if output.len() < packet_len {
                        defmt::error!(
                            "Output buffer too small! {:?} < {:?}",
                            output.len(),
                            packet_len
                        );
                        return 0;
                    }
                    output[..packet_len].copy_from_slice(&a[..packet_len]);
                    return packet_len;
                }
            }
            _ => return 0,
        }
    }
    // Continuation byte == 1 four times, that's illegal.
    0
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

    impl TcpStack for MockNetwork {
        type TcpSocket = ();
        type Error = ();

        fn open(&self, _mode: embedded_nal::Mode) -> Result<Self::TcpSocket, Self::Error> {
            Ok(())
        }
        fn connect(
            &self,
            _socket: Self::TcpSocket,
            _remote: embedded_nal::SocketAddr,
        ) -> Result<Self::TcpSocket, Self::Error> {
            Ok(())
        }
        fn is_connected(&self, _socket: &Self::TcpSocket) -> Result<bool, Self::Error> {
            Ok(true)
        }
        fn write(
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
        fn read(
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

        fn read_with<F>(&self, socket: &mut Self::TcpSocket, f: F) -> nb::Result<usize, Self::Error>
        where
            F: FnOnce(&[u8], Option<&[u8]>) -> usize,
        {
            let buf = &mut [0u8; 64];
            self.read(socket, buf)?;
            Ok(f(buf, None))
        }

        fn close(&self, _socket: Self::TcpSocket) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn test_parse_header_puback() {
        let mut out = [0u8; 128];
        let a = &[0b01000000, 0b00000010, 0, 10];

        let len = parse_header(a, None, &mut out);
        assert_eq!(len, 4);
        assert_eq!(&out[..len], &a[..]);
        assert_eq!(
            decode_slice(&out[..len]).unwrap(),
            Some(Packet::Puback(Pid::try_from(10).unwrap()))
        );
    }

    #[test]
    fn test_parse_header_simple() {
        let mut out = [0u8; 128];
        let a = &[
            0b00010000, 39, 0x00, 0x04, 'M' as u8, 'Q' as u8, 'T' as u8, 'T' as u8, 0x04,
            0b11001110, // +username, +password, -will retain, will qos=1, +last_will, +clean_session
            0x00, 0x0a, // 10 sec
            0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8, 't' as u8, // client_id
            0x00, 0x02, '/' as u8, 'a' as u8, // will topic = '/a'
            0x00, 0x07, 'o' as u8, 'f' as u8, 'f' as u8, 'l' as u8, 'i' as u8, 'n' as u8,
            'e' as u8, // will msg = 'offline'
            0x00, 0x04, 'r' as u8, 'u' as u8, 's' as u8, 't' as u8, // username = 'rust'
            0x00, 0x02, 'm' as u8, 'q' as u8, // password = 'mq'
        ];

        let len = parse_header(a, None, &mut out);
        assert_eq!(len, 41);
        assert_eq!(&out[..len], &a[..]);
        assert_eq!(
            decode_slice(&out[..len]).unwrap(),
            Some(Packet::Connect(Connect {
                protocol: Protocol::MQTT311,
                keep_alive: 10,
                client_id: "test".into(),
                clean_session: true,
                last_will: Some(mqttrs::LastWill {
                    topic: "/a".into(),
                    message: b"offline",
                    qos: QoS::AtLeastOnce,
                    retain: false,
                }),
                username: Some("rust".into()),
                password: Some(b"mq"),
            }))
        );
    }

    #[test]
    fn test_parse_header_additional() {
        let mut out = [0u8; 128];
        let a = &[
            0b00010000, 39, 0x00, 0x04, 'M' as u8, 'Q' as u8, 'T' as u8, 'T' as u8, 0x04,
            0b11001110, // +username, +password, -will retain, will qos=1, +last_will, +clean_session
            0x00, 0x0a, // 10 sec
            0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8, 't' as u8, // client_id
            0x00, 0x02, '/' as u8, 'a' as u8, // will topic = '/a'
            0x00, 0x07, 'o' as u8, 'f' as u8, 'f' as u8, 'l' as u8, 'i' as u8, 'n' as u8,
            'e' as u8, // will msg = 'offline'
            0x00, 0x04, 'r' as u8, 'u' as u8, 's' as u8, 't' as u8, // username = 'rust'
            0x00, 0x02, 'm' as u8, 'q' as u8, // password = 'mq'
            0x00, 0x01, // additional bytes
            0x00, 0x01, // additional bytes
            0x00, 0x01, // additional bytes
        ];

        let len = parse_header(a, None, &mut out);
        assert_eq!(len, 41);
        assert_eq!(&out[..len], &a[..len]);
        assert_eq!(
            decode_slice(&out[..len]).unwrap(),
            Some(Packet::Connect(Connect {
                protocol: Protocol::MQTT311,
                keep_alive: 10,
                client_id: "test".into(),
                clean_session: true,
                last_will: Some(mqttrs::LastWill {
                    topic: "/a".into(),
                    message: b"offline",
                    qos: QoS::AtLeastOnce,
                    retain: false,
                }),
                username: Some("rust".into()),
                password: Some(b"mq"),
            }))
        );
    }

    #[test]
    fn test_parse_header_wrapped_simple() {
        let mut out = [0u8; 128];
        let a = &[
            0b00010000, 39, 0x00, 0x04, 'M' as u8, 'Q' as u8, 'T' as u8, 'T' as u8, 0x04,
        ];

        let b = &[
            0b11001110, // +username, +password, -will retain, will qos=1, +last_will, +clean_session
            0x00, 0x0a, // 10 sec
            0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8, 't' as u8, // client_id
            0x00, 0x02, '/' as u8, 'a' as u8, // will topic = '/a'
            0x00, 0x07, 'o' as u8, 'f' as u8, 'f' as u8, 'l' as u8, 'i' as u8, 'n' as u8,
            'e' as u8, // will msg = 'offline'
            0x00, 0x04, 'r' as u8, 'u' as u8, 's' as u8, 't' as u8, // username = 'rust'
            0x00, 0x02, 'm' as u8, 'q' as u8, // password = 'mq'
            0x00, 0x01, // additional bytes
            0x00, 0x01, // additional bytes
            0x00, 0x01, // additional bytes
        ];

        let expected = &[
            0b00010000, 39, 0x00, 0x04, 'M' as u8, 'Q' as u8, 'T' as u8, 'T' as u8, 0x04,
            0b11001110, // +username, +password, -will retain, will qos=1, +last_will, +clean_session
            0x00, 0x0a, // 10 sec
            0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8, 't' as u8, // client_id
            0x00, 0x02, '/' as u8, 'a' as u8, // will topic = '/a'
            0x00, 0x07, 'o' as u8, 'f' as u8, 'f' as u8, 'l' as u8, 'i' as u8, 'n' as u8,
            'e' as u8, // will msg = 'offline'
            0x00, 0x04, 'r' as u8, 'u' as u8, 's' as u8, 't' as u8, // username = 'rust'
            0x00, 0x02, 'm' as u8, 'q' as u8, // password = 'mq'
        ];

        let len = parse_header(a, Some(b), &mut out);
        assert_eq!(len, 41);
        assert_eq!(&out[..len], &expected[..]);
        assert_eq!(
            decode_slice(&out[..len]).unwrap(),
            Some(Packet::Connect(Connect {
                protocol: Protocol::MQTT311,
                keep_alive: 10,
                client_id: "test".into(),
                clean_session: true,
                last_will: Some(mqttrs::LastWill {
                    topic: "/a".into(),
                    message: b"offline",
                    qos: QoS::AtLeastOnce,
                    retain: false,
                }),
                username: Some("rust".into()),
                password: Some(b"mq"),
            }))
        );
    }

    #[test]
    fn test_parse_header_wrapped_header() {
        let mut out = [0u8; 128];
        let a = &[0b00010000];

        let b = &[
            39, 0x00, 0x04, 'M' as u8, 'Q' as u8, 'T' as u8, 'T' as u8, 0x04,
            0b11001110, // +username, +password, -will retain, will qos=1, +last_will, +clean_session
            0x00, 0x0a, // 10 sec
            0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8, 't' as u8, // client_id
            0x00, 0x02, '/' as u8, 'a' as u8, // will topic = '/a'
            0x00, 0x07, 'o' as u8, 'f' as u8, 'f' as u8, 'l' as u8, 'i' as u8, 'n' as u8,
            'e' as u8, // will msg = 'offline'
            0x00, 0x04, 'r' as u8, 'u' as u8, 's' as u8, 't' as u8, // username = 'rust'
            0x00, 0x02, 'm' as u8, 'q' as u8, // password = 'mq'
            0x00, 0x01, // additional bytes
            0x00, 0x01, // additional bytes
            0x00, 0x01, // additional bytes
        ];

        let expected = &[
            0b00010000, 39, 0x00, 0x04, 'M' as u8, 'Q' as u8, 'T' as u8, 'T' as u8, 0x04,
            0b11001110, // +username, +password, -will retain, will qos=1, +last_will, +clean_session
            0x00, 0x0a, // 10 sec
            0x00, 0x04, 't' as u8, 'e' as u8, 's' as u8, 't' as u8, // client_id
            0x00, 0x02, '/' as u8, 'a' as u8, // will topic = '/a'
            0x00, 0x07, 'o' as u8, 'f' as u8, 'f' as u8, 'l' as u8, 'i' as u8, 'n' as u8,
            'e' as u8, // will msg = 'offline'
            0x00, 0x04, 'r' as u8, 'u' as u8, 's' as u8, 't' as u8, // username = 'rust'
            0x00, 0x02, 'm' as u8, 'q' as u8, // password = 'mq'
        ];

        let len = parse_header(a, Some(b), &mut out);
        assert_eq!(len, 41);
        assert_eq!(&out[..len], &expected[..]);
        assert_eq!(
            decode_slice(&out[..len]).unwrap(),
            Some(Packet::Connect(Connect {
                protocol: Protocol::MQTT311,
                keep_alive: 10,
                client_id: "test".into(),
                clean_session: true,
                last_will: Some(mqttrs::LastWill {
                    topic: "/a".into(),
                    message: b"offline",
                    qos: QoS::AtLeastOnce,
                    retain: false,
                }),
                username: Some("rust".into()),
                password: Some(b"mq"),
            }))
        );
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
