use crate::options::Broker;
use crate::requests::{PublishPayload, Request};
use crate::state::{MqttConnectionStatus, MqttState};
use crate::MqttOptions;
use crate::{EventError, NetworkError, Notification};
use core::convert::Infallible;
use core::ops::{Add, RangeTo};
use embedded_nal::{AddrType, Dns, TcpClient};
use embedded_time::duration::Extensions;
use embedded_time::duration::Milliseconds;
use embedded_time::{Clock, Instant};
use heapless::{consts, spsc, ArrayLength, String, Vec};
use mqttrs::{decode_slice, encode_slice, Connect, Packet, Protocol, QoS};
use no_std_net::SocketAddr;

pub struct EventLoop<'a, 'b, L, S, O, P>
where
    L: ArrayLength<Request<P>>,
    O: Clock,
    P: PublishPayload,
{
    /// Current state of the connection
    pub state: MqttState<P, Instant<O>>,
    /// Last outgoing packet time
    pub last_outgoing_timer: O,
    /// Options of the current mqtt connection
    pub options: MqttOptions<'b>,
    /// Request stream
    pub requests: spsc::Consumer<'a, Request<P>, L, u8>,
    network_handle: NetworkHandle<S>,
    rx_buf: PacketBuffer,
}

impl<'a, 'b, L, S, O, P> EventLoop<'a, 'b, L, S, O, P>
where
    L: ArrayLength<Request<P>>,
    O: Clock,
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
            requests,
            network_handle: NetworkHandle::new(),
            rx_buf: PacketBuffer::new(),
        }
    }

    pub fn connect<N: Dns + TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<bool, EventError> {
        use EventError::*;

        // connect to the broker
        if !self.is_connected(network).map_err(|e| {
            // On the socket error, clean it up and bail out.
            self.network_handle.socket.take();
            EventError::Network(e)
        })? {
            let (broker, port) = self.options.broker();
            let (_hostname, socket_addr) =
                Self::lookup_host(network, broker, port).map_err(EventError::Network)?;
            self.network_connect(network, socket_addr)
                .map_err(EventError::Network)?;
            self.state.connection_status = MqttConnectionStatus::Disconnected;
        }

        self.mqtt_connect(network).map_err(|e| {
            e.map(|e| {
                if matches!(e, Network(_) | MqttState(_) | Timeout) {
                    defmt::debug!(
                        "Disconnecting from {:?}",
                        defmt::Debug2Format::<consts::U64>(&e)
                    );
                    self.disconnect(network);
                }
                e
            })
        })
    }

    fn should_handle_request(&mut self) -> bool {
        let qos_space = self.state.outgoing_pub.len() < self.options.inflight();

        let qos_0 = if let Some(Request::Publish(p)) = self.requests.peek() {
            p.qos == QoS::AtMostOnce
        } else {
            false
        };

        qos_0 || (self.requests.ready() && qos_space)
    }

    /// Selects an event from the client's requests, incoming packets from the
    /// broker and keepalive ping cycle.
    fn select_event<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<Notification, EventError> {
        let packet_buf = &mut [0_u8; 1024];

        let now = self
            .last_outgoing_timer
            .try_now()
            .map_err(EventError::from)?;

        if self.should_handle_request() {
            // Handle a request
            let request = unsafe { self.requests.dequeue_unchecked() };
            let packet = self
                .state
                .handle_outgoing_request(request, packet_buf, &now)
                .map_err(EventError::from)?;
            self.network_handle.send(network, &packet)?;
            return Err(nb::Error::WouldBlock);
        }

        if self
            .state
            .last_ping_entry()
            .or_insert(now)
            .has_elapsed(&now, self.options.keep_alive_ms().milliseconds())
        {
            // Handle keepalive ping
            let packet = self
                .state
                .handle_outgoing_packet(Packet::Pingreq)
                .map_err(EventError::from)?;
            self.network_handle.send(network, &packet)?;
            self.state.last_ping_entry().insert(now);
            return Err(nb::Error::WouldBlock);
        }

        // Handle an incoming packet
        let (notification, packet) = self.receive(network)?;

        if let Some(packet) = packet {
            self.network_handle.send(network, &packet)?;
        }

        // By comparing the current time, select pending non-zero QoS publish
        // requests staying longer than the retry interval, and handle their
        // retrial.
        for (pid, inflight) in self.state.retries(now, 50_000.milliseconds()) {
            let packet = inflight
                .packet(*pid, packet_buf)
                .map_err(EventError::from)?;
            // Update inflight's timestamp for later retrials
            inflight.last_touch_entry().insert(now);
            self.network_handle.send(network, &packet)?;
        }

        notification.ok_or(nb::Error::WouldBlock)
    }

    /// Yields notification from events. All the error raised while processing
    /// event is reported as an `Ok` value of `Notification::Abort`.
    #[must_use = "Eventloop should be iterated over a loop to make progress"]
    pub fn yield_event<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<Notification, Infallible> {
        if self.network_handle.socket.is_none() {
            return Ok(Notification::Abort(EventError::Network(
                NetworkError::NoSocket,
            )));
        }

        self.select_event(network).or_else(|e| match e {
            nb::Error::WouldBlock => Err(nb::Error::WouldBlock),
            nb::Error::Other(e) => {
                defmt::debug!(
                    "Disconnecting from an event error. {:?}",
                    defmt::Debug2Format::<consts::U64>(&e)
                );
                self.disconnect(network);
                Ok(Notification::Abort(e))
            }
        })
    }

    pub fn receive<N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
    ) -> nb::Result<(Option<Notification>, Option<Packet<'static>>), EventError> {
        let socket = self
            .network_handle
            .socket
            .as_mut()
            .ok_or(EventError::Network(NetworkError::NoSocket))?;

        self.rx_buf.receive(socket, network)?;

        PacketDecoder::new(&mut self.rx_buf).decode(&mut self.state)
    }

    fn lookup_host<N: Dns + TcpClient<TcpSocket = S>>(
        network: &N,
        broker: Broker,
        port: u16,
    ) -> Result<(String<consts::U256>, SocketAddr), NetworkError> {
        match broker {
            Broker::Hostname(h) => {
                let socket_addr = SocketAddr::new(
                    network.gethostbyname(h, AddrType::IPv4).map_err(|_e| {
                        defmt::info!("Failed to resolve IP!");
                        NetworkError::DnsLookupFailed
                    })?,
                    port,
                );
                Ok((String::from(h), socket_addr))
            }
            Broker::IpAddr(ip) => {
                let socket_addr = SocketAddr::new(ip, port);
                let domain = network.gethostbyaddr(ip).map_err(|_e| {
                    defmt::info!("Failed to resolve hostname!");
                    NetworkError::DnsLookupFailed
                })?;

                Ok((domain, socket_addr))
            }
        }
    }

    /// Checks if this socket is present and connected. Raises `NetworkError` when
    /// the socket is present and in its error state.
    fn is_connected<N: Dns + TcpClient<TcpSocket = S>>(
        &self,
        network: &N,
    ) -> Result<bool, NetworkError> {
        self.network_handle
            .socket
            .as_ref()
            .map_or(Ok(false), |socket| network.is_connected(&socket))
            .map_err(|_e| NetworkError::SocketClosed)
    }

    fn network_connect<N: Dns + TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
        socket_addr: SocketAddr,
    ) -> Result<(), NetworkError> {
        self.network_handle.socket = network
            .socket()
            .map_err(|_e| NetworkError::SocketOpen)?
            .into();

        network
            .connect(
                self.network_handle
                    .socket
                    .as_mut()
                    .unwrap_or_else(|| unreachable!()),
                socket_addr,
            )
            .map_err(|_e| NetworkError::SocketConnect)?;

        defmt::debug!("Network connected!");

        Ok(())
    }

    pub fn disconnect<N: TcpClient<TcpSocket = S>>(&mut self, network: &N) {
        self.state.connection_status = MqttConnectionStatus::Disconnected;
        if let Some(socket) = self.network_handle.socket.take() {
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
                let now = self
                    .last_outgoing_timer
                    .try_now()
                    .map_err(EventError::from)?;
                self.state.last_ping_entry().insert(now);

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
                self.network_handle.send(network, &connect.into())?;
                self.state.handle_outgoing_connect();

                Err(nb::Error::WouldBlock)
            }
            MqttConnectionStatus::Handshake => {
                let now = self
                    .last_outgoing_timer
                    .try_now()
                    .map_err(EventError::from)?;
                if self
                    .state
                    .last_ping_entry()
                    .or_insert(now)
                    .has_elapsed(&now, 50_000.milliseconds())
                {
                    return Err(nb::Error::Other(EventError::Timeout));
                }

                self.receive(network).and_then(|(n, p)| {
                    if n.is_none() && p.is_none() {
                        return Err(nb::Error::WouldBlock);
                    }
                    Ok(n.map(|n| n == Notification::ConnAck).unwrap_or(false))
                })
            }
        }
    }
}

struct NetworkHandle<S> {
    /// Network socket
    socket: Option<S>,
    tx_buf: Vec<u8, consts::U1024>,
}

impl<S> NetworkHandle<S> {
    fn new() -> Self {
        Self {
            socket: None,
            tx_buf: Vec::new(),
        }
    }

    pub fn send<'d, N: TcpClient<TcpSocket = S>>(
        &mut self,
        network: &N,
        pkt: &Packet<'d>,
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

        Ok(length)
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
        let len = network.receive(socket, buffer).map_err(|_| {
            defmt::error!("[receive] NetworkError::Read");
            EventError::Network(NetworkError::Read)
        })?;
        self.range.end += len;
        Ok(())
    }
}

/// Provides contextual information for decoding packets. If an incoming packet
/// is well-formed and has a packet type the underlying state expects, returns a
/// notification. On an error, cleans up its buffer state.
struct PacketDecoder<'a> {
    packet_buffer: &'a mut PacketBuffer,
    is_err: Option<bool>,
}

impl<'a> PacketDecoder<'a> {
    fn new(packet_buffer: &'a mut PacketBuffer) -> Self {
        Self {
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

    fn decode<P, T>(
        mut self,
        state: &mut MqttState<P, T>,
    ) -> nb::Result<(Option<Notification>, Option<Packet<'static>>), EventError>
    where
        P: PublishPayload + Clone,
        T: Add<Milliseconds, Output = T> + PartialOrd + Copy,
    {
        let buffer = self.packet_buffer.buffer[self.packet_buffer.range].as_ref();
        match decode_slice(buffer) {
            Err(e) => {
                self.is_err.replace(true);
                defmt::error!(
                    "Packet decode error! {:?}",
                    defmt::Debug2Format::<heapless::consts::U64>(&e)
                );

                Err(EventError::Encoding(e).into())
            }
            Ok(Some(packet)) => {
                self.is_err.replace(false);
                state
                    .handle_incoming_packet(packet)
                    .map_err(EventError::from)
                    .map_err(nb::Error::from)
            }
            Ok(None) => Err(nb::Error::WouldBlock),
        }
    }
}

impl<'a> Drop for PacketDecoder<'a> {
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
    use crate::state::{Inflight, StartTime};
    use crate::PublishRequest;
    use embedded_time::clock::Error;
    use embedded_time::duration::Milliseconds;
    use embedded_time::fraction::Fraction;
    use embedded_time::Instant;
    use heapless::{consts, spsc::Queue, String, Vec};
    use mqttrs::Error::InvalidConnectReturnCode;
    use mqttrs::{Connack, ConnectReturnCode, Pid, Publish, QosPid};

    #[derive(Debug)]
    struct ClockMock {
        time: u32,
    }

    impl Clock for ClockMock {
        const SCALING_FACTOR: Fraction = Fraction::new(1000, 1);
        type T = u32;

        fn try_now(&self) -> Result<Instant<Self>, Error> {
            Ok(Instant::new(self.time))
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
        let mut state = MqttState::<Vec<u8, consts::U10>, Milliseconds>::new();
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
        let (n, p) = PacketDecoder::new(&mut rx_buf).decode(&mut state).unwrap();
        assert_eq!(n, Some(Notification::ConnAck));
        assert_eq!(p, None);

        // Decode the second Publish packet on the Connected state.
        assert_eq!(state.connection_status, MqttConnectionStatus::Connected);
        let (n, p) = PacketDecoder::new(&mut rx_buf).decode(&mut state).unwrap();
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
        let mut state = MqttState::<Vec<u8, consts::U10>, Milliseconds>::new();
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
        match PacketDecoder::new(&mut rx_buf).decode(&mut state) {
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
            ClockMock { time: 0 },
            MqttOptions::new("client", Broker::Hostname(""), 8883),
        );

        let now = StartTime::from(0);

        event
            .state
            .outgoing_pub
            .insert(
                2,
                Inflight::new(
                    now,
                    PublishRequest {
                        dup: false,
                        qos: QoS::AtLeastOnce,
                        retain: false,
                        topic_name: String::from("some/topic/name2"),
                        payload: Vec::new(),
                    },
                ),
            )
            .unwrap();

        event
            .state
            .outgoing_pub
            .insert(
                3,
                Inflight::new(
                    now,
                    PublishRequest {
                        dup: false,
                        qos: QoS::AtLeastOnce,
                        retain: false,
                        topic_name: String::from("some/topic/name3"),
                        payload: Vec::new(),
                    },
                ),
            )
            .unwrap();

        event
            .state
            .outgoing_pub
            .insert(
                4,
                Inflight::new(
                    now,
                    PublishRequest {
                        dup: false,
                        qos: QoS::AtLeastOnce,
                        retain: false,
                        topic_name: String::from("some/topic/name4"),
                        payload: Vec::new(),
                    },
                ),
            )
            .unwrap();

        event.state.connection_status = MqttConnectionStatus::Handshake;
        event.network_handle.socket = Some(());

        event.connect(&network).unwrap();
    }
}
