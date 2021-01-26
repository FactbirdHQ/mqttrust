use crate::options::Broker;
use crate::requests::{PublishPayload, Request};
use crate::state::{MqttConnectionStatus, MqttState};
use crate::MqttOptions;
use crate::{EventError, NetworkError, Notification};
use core::convert::{Infallible, TryInto};
use core::ops::RangeTo;
use embedded_nal::{AddrType, Dns, TcpClient};
use heapless::{consts, spsc, ArrayLength, Vec};
use mqttrs::{decode_slice, encode_slice, Connect, Packet, Protocol, QoS};
use no_std_net::SocketAddr;

pub struct EventLoop<'a, 'b, L, S, O, P>
where
    L: ArrayLength<Request<P>>,
    P: PublishPayload,
{
    /// Current state of the connection
    pub state: MqttState<P>,
    /// Last outgoing packet time
    pub last_outgoing_timer: O,
    /// Options of the current mqtt connection
    pub options: MqttOptions<'b>,
    /// Network socket
    pub socket: Option<S>,
    /// Request stream
    pub requests: spsc::Consumer<'a, Request<P>, L, u8>,
    tx_buf: Vec<u8, consts::U1024>,
    rx_buf: PacketBuffer,
}

impl<'a, 'b, L, S, O, P> EventLoop<'a, 'b, L, S, O, P>
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
        Self {
            state: MqttState::new(),
            last_outgoing_timer: outgoing_timer,
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
        self.mqtt_connect(network)
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

    /// Selects an event from the client's requests, incoming packets from the
    /// broker and keepalive ping cycle.
    fn select_event<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<Notification, EventError> {
        let packet_buf = &mut [0_u8; 1024];

        if self.should_handle_request() {
            // Handle a request
            let request = unsafe { self.requests.dequeue_unchecked() };
            let packet = self
                .state
                .handle_outgoing_request(request, packet_buf)
                .map_err(EventError::from)?;
            self.send(network, packet)?;
            return Err(nb::Error::WouldBlock);
        }

        if self.last_outgoing_timer.try_wait().is_ok() {
            // Handle keepalive ping
            let packet = self
                .state
                .handle_outgoing_packet(Packet::Pingreq)
                .map_err(EventError::from)?;
            self.send(network, packet)?;
            return Err(nb::Error::WouldBlock);
        }

        // Handle an incoming packet
        let (notification, packet) = self.receive(network)?;

        if let Some(packet) = packet {
            self.send(network, packet)?;
        }

        notification.ok_or(nb::Error::WouldBlock)
    }

    /// Yields notification from events. All the error raised while processing
    /// event is reported as an `Ok` value of `Notification::Abort`.
    pub fn yield_event<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<Notification, Infallible> {
        self.select_event(network).or_else(|e| match e {
            nb::Error::WouldBlock => Err(nb::Error::WouldBlock),
            nb::Error::Other(e) => {
                defmt::debug!("Disconnecting from an event error.");
                self.disconnect(network);
                Ok(Notification::Abort(e))
            }
        })
    }

    pub fn send<'d, N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
        pkt: Packet<'d>,
    ) -> Result<usize, EventError> {
        let socket = self
            .socket
            .as_mut()
            .ok_or(EventError::Network(NetworkError::NoSocket))?;
        let capacity = self.tx_buf.capacity();
        self.tx_buf.clear();
        self.tx_buf
            .resize(capacity, 0x00_u8)
            .unwrap_or_else(|()| unreachable!("Input length equals to the current capacity."));
        let size = encode_slice(&pkt, self.tx_buf.as_mut())?;
        let length = nb::block!(network.send(socket, &self.tx_buf[..size])).map_err(|_| {
            defmt::error!("[send] NetworkError::Write");
            EventError::Network(NetworkError::Write)
        })?;

        let last_outgoing_duration =
            if self.state.connection_status == MqttConnectionStatus::Disconnected {
                5000
            } else {
                self.options.keep_alive_ms()
            };

        self.last_outgoing_timer
            .try_start(last_outgoing_duration)
            .ok();

        Ok(length)
    }

    pub fn receive<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<(Option<Notification>, Option<Packet<'static>>), EventError> {
        let socket = self
            .socket
            .as_mut()
            .ok_or(EventError::Network(NetworkError::NoSocket))?;

        self.rx_buf.receive(socket, network)?;

        PacketDecoder::new(&mut self.state, &mut self.rx_buf).try_into()
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

        self.state.connection_status = MqttConnectionStatus::Disconnected;

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
        self.state.connection_status = MqttConnectionStatus::Disconnected;
        if let Some(socket) = self.socket.take() {
            network.close(socket).ok();
        }
    }

    fn mqtt_connect<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<bool, EventError> {
        match self.state.connection_status {
            MqttConnectionStatus::Connected => Ok(false),
            MqttConnectionStatus::Disconnected => {
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

                        self.last_outgoing_timer.try_start(50000).ok();
                    }
                    Err(e) => {
                        defmt::debug!("Disconnecting from send error!");
                        self.disconnect(network);
                        return Err(nb::Error::Other(e));
                    }
                }

                Err(nb::Error::WouldBlock)
            }
            MqttConnectionStatus::Handshake => {
                if self.last_outgoing_timer.try_wait().is_ok() {
                    defmt::debug!("Disconnecting from handshake timeout!");
                    self.disconnect(network);
                    return Err(nb::Error::Other(EventError::Timeout));
                }

                self.receive(network).and_then(|(n, p)| {
                    if n.is_none() && p.is_none() {
                        return Err(nb::Error::WouldBlock);
                    }
                    Ok(n.map(|n| n == Notification::ConnAck).unwrap_or(false))
                }).map_err(|e| {
                    if let nb::Error::Other(_) = e {
                        self.disconnect(network);
                    }
                    e
                })
            }
        }
    }
}

/// A placeholder that keeps a buffer and constructs a packet incrementally.
/// Given that underlying `TcpClient` throws `WouldBlock` in a non-blocking
/// manner, its packet construction won't block either.
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

    /// Fills the buffer with all 0s
    fn init(&mut self) {
        self.range.end = 0;
        self.buffer.clear();
        self.buffer
            .resize(self.buffer.capacity(), 0x00u8)
            .unwrap_or_else(|()| unreachable!("Length equals to the current capacity."));
    }

    /// Returns a remaining fresh part of the buffer.
    fn buffer(&mut self) -> &mut [u8] {
        let range = self.range.end..;
        self.buffer[range].as_mut()
    }

    /// After decoding a packet, overwrite the used bytes by shifting the buffer
    /// by its length. Assumes the length fits within the buffer's capacity.
    fn rotate(&mut self, length: usize) {
        self.buffer.copy_within(length.., 0);
        self.range.end -= length;
        self.buffer.truncate(self.buffer.capacity() - length);
        self.buffer
            .resize(self.buffer.capacity(), 0)
            .unwrap_or_else(|()| unreachable!("Length equals to the current capacity."));
    }

    /// Receives bytes from a network socket in non-blocking mode. If incoming
    /// bytes found, the range gets extended covering them.
    fn receive<N, S>(&mut self, socket: &mut S, network: &N) -> nb::Result<(), EventError>
    where
        N: TcpClient<TcpSocket = S>,
    {
        let buffer = self.buffer();
        let len = network.receive(socket, buffer).map_err(|e| {
            e.map(|_| {
                defmt::error!("[receive] NetworkError::Read");
                EventError::Network(NetworkError::Read)
            })
        })?;
        self.range.end += len;
        Ok(())
    }
}

/// Provides contextual information for decoding packets. If an incoming packet
/// is well-formed and has a packet type the underlying state expects, returns a
/// notification. On an error, cleans up its buffer state.
struct PacketDecoder<'a, P>
where
    P: PublishPayload + Clone,
{
    state: &'a mut MqttState<P>,
    packet_buffer: &'a mut PacketBuffer,
    is_err: Option<bool>,
}

impl<'a, P> PacketDecoder<'a, P>
where
    P: PublishPayload + Clone,
{
    fn new(state: &'a mut MqttState<P>, packet_buffer: &'a mut PacketBuffer) -> Self {
        Self {
            state,
            packet_buffer,
            is_err: None,
        }
    }

    // https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718023
    fn packet_length(&self) -> Option<usize> {
        // The result of earlier decode_slice failed with an error or incomplete
        // packet.
        if self.is_err.unwrap_or(true) {
            return None;
        }

        // The buffer contains a valid packet.
        self.packet_buffer
            .buffer
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
            .into()
    }
}

impl<'a, P> TryInto<(Option<Notification>, Option<Packet<'static>>)> for PacketDecoder<'a, P>
where
    P: PublishPayload + Clone,
{
    type Error = nb::Error<EventError>;
    fn try_into(mut self) -> Result<(Option<Notification>, Option<Packet<'static>>), Self::Error> {
        let buffer = self.packet_buffer.buffer[self.packet_buffer.range].as_ref();
        match decode_slice(buffer) {
            Err(e) => {
                self.is_err.replace(true);
                Err(EventError::Encoding(e).into())
            }
            Ok(Some(packet)) => {
                self.is_err.replace(false);
                self.state
                    .handle_incoming_packet(packet)
                    .map_err(EventError::from)
                    .map_err(nb::Error::from)
            }
            Ok(None) => Err(nb::Error::WouldBlock),
        }
    }
}

impl<'a, P> Drop for PacketDecoder<'a, P>
where
    P: PublishPayload + Clone,
{
    fn drop(&mut self) {
        if let Some(is_err) = self.is_err {
            if is_err {
                self.packet_buffer.init();
            } else {
                let length = self
                    .packet_length()
                    .unwrap_or_else(|| unreachable!("A valid packet has a non-zero length."));
                self.packet_buffer.rotate(length);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PublishRequest;
    use embedded_hal::timer::CountDown;
    use heapless::{consts, spsc::Queue, String, Vec};
    use mqttrs::Error::InvalidConnectReturnCode;
    use mqttrs::{Connack, ConnectReturnCode, Pid, Publish, QosPid};

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
    fn success_receive_multiple_packets() {
        let mut state = MqttState::<Vec<u8, consts::U10>>::new();
        let mut rx_buf = PacketBuffer::new();
        let connack = Connack {
            session_present: false,
            code: ConnectReturnCode::Accepted,
        };
        let publish = Publish {
            dup: false,
            qospid: QosPid::AtLeastOnce(Pid::new()),
            retain: false,
            topic_name: "test/topic",
            payload: &[0xff; 1003],
        };

        let connack_len = encode_slice(&Packet::from(connack), rx_buf.buffer()).unwrap();
        rx_buf.range.end += connack_len;
        let publish_len = encode_slice(&Packet::from(publish.clone()), rx_buf.buffer()).unwrap();
        rx_buf.range.end += publish_len;
        assert_eq!(rx_buf.range.end, rx_buf.buffer.capacity());

        // Decode the first Connack packet on the Handshake state.
        state.connection_status = MqttConnectionStatus::Handshake;
        let (n, p) = PacketDecoder::new(&mut state, &mut rx_buf)
            .try_into()
            .unwrap();
        assert_eq!(n, Some(Notification::ConnAck));
        assert_eq!(p, None);

        // Decode the second Publish packet on the Connected state.
        assert_eq!(state.connection_status, MqttConnectionStatus::Connected);
        let (n, p) = PacketDecoder::new(&mut state, &mut rx_buf)
            .try_into()
            .unwrap();
        let publish_notification = match n {
            Some(Notification::Publish(p)) => p,
            _ => panic!(),
        };
        assert_eq!(&publish_notification.payload, publish.payload);
        assert_eq!(p, Some(Packet::Puback(Pid::default())));
        assert_eq!(rx_buf.range.end, 0);
        assert!((0..1024).all(|i| rx_buf.buffer[i] == 0));
    }

    #[test]
    fn failure_receive_multiple_packets() {
        let mut state = MqttState::<Vec<u8, consts::U10>>::new();
        let mut rx_buf = PacketBuffer::new();
        let connack_malformed = Connack {
            session_present: false,
            code: ConnectReturnCode::Accepted,
        };
        let publish = Publish {
            dup: false,
            qospid: QosPid::AtLeastOnce(Pid::new()),
            retain: false,
            topic_name: "test/topic",
            payload: &[0xff; 1003],
        };

        let connack_malformed_len =
            encode_slice(&Packet::from(connack_malformed), rx_buf.buffer()).unwrap();
        rx_buf.buffer()[3] = 6; // An invalid connect return code.
        rx_buf.range.end += connack_malformed_len;
        let publish_len = encode_slice(&Packet::from(publish.clone()), rx_buf.buffer()).unwrap();
        rx_buf.range.end += publish_len;
        assert_eq!(rx_buf.range.end, rx_buf.buffer.capacity());

        // When a packet is malformed, we cannot tell its length. The decoder
        // discards the entire buffer.
        state.connection_status = MqttConnectionStatus::Handshake;
        match PacketDecoder::new(&mut state, &mut rx_buf).try_into() {
            Ok((_, _)) | Err(nb::Error::WouldBlock) => panic!(),
            Err(nb::Error::Other(e)) => {
                assert_eq!(e, EventError::Encoding(InvalidConnectReturnCode(6)))
            }
        }
        assert_eq!(state.connection_status, MqttConnectionStatus::Handshake);
        assert_eq!(rx_buf.range.end, 0);
        assert!((0..1024).all(|i| rx_buf.buffer[i] == 0));
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
        let mut event = EventLoop::<_, (), _, _>::new(
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

        event.state.connection_status = MqttConnectionStatus::Handshake;
        event.socket = Some(());

        event.connect(&network).unwrap();
    }
}
