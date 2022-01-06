use crate::options::Broker;
use crate::packet::SerializedPacket;
use crate::state::{MqttConnectionStatus, MqttState};
use crate::{mqtt_log, EventError, MqttOptions, NetworkError, Notification};
use bbqueue::framed::FrameConsumer;
use core::convert::Infallible;
use core::ops::DerefMut;
use core::ops::{Add, RangeTo};
use embedded_nal::{AddrType, Dns, SocketAddr, TcpClientStack};
use embedded_time::duration::Extensions;
use embedded_time::duration::Milliseconds;
use embedded_time::{Clock, Instant};
use heapless::{String, Vec};
use mqttrust::encoding::v4::{decode_slice, encode_slice, Connect, Packet, Protocol};

pub struct EventLoop<'a, 'b, S, O, const L: usize>
where
    O: Clock,
{
    /// Current state of the connection
    pub(crate) state: MqttState<Instant<O>>,
    /// Last outgoing packet time
    pub(crate) last_outgoing_timer: O,
    /// Options of the current mqtt connection
    pub(crate) options: MqttOptions<'b>,
    /// Request stream
    pub(crate) requests: FrameConsumer<'a, L>,
    network_handle: NetworkHandle<S>,
}

impl<'a, 'b, S, O, const L: usize> EventLoop<'a, 'b, S, O, L>
where
    O: Clock,
{
    pub fn new(
        requests: FrameConsumer<'a, L>,
        outgoing_timer: O,
        options: MqttOptions<'b>,
    ) -> Self {
        Self {
            state: MqttState::new(),
            last_outgoing_timer: outgoing_timer,
            options,
            requests,
            network_handle: NetworkHandle::new(),
        }
    }

    pub fn connect<N: Dns + TcpClientStack<TcpSocket = S> + ?Sized>(
        &mut self,
        network: &mut N,
    ) -> nb::Result<bool, EventError> {
        // connect to the broker
        match self.network_handle.is_connected(network) {
            Ok(false) => {
                // Socket is present, but not connected. Usually this implies
                // that the socket is closed for writes. Disconnect to close &
                // recycle the socket.
                mqtt_log!(warn, "Socket cleanup!");
                self.disconnect(network);
                return Err(EventError::Network(NetworkError::SocketClosed).into());
            }
            Err(_) => {
                // We have no socket present at all
                let (broker, port) = self.options.broker();
                let (_hostname, socket_addr) =
                    NetworkHandle::<S>::lookup_host(network, broker, port)
                        .map_err(EventError::Network)?;
                self.network_handle
                    .connect(network, socket_addr)
                    .map_err(EventError::Network)?;
                mqtt_log!(debug, "Network connected!");

                self.state.connection_status = MqttConnectionStatus::Disconnected;
            }
            Ok(true) => {
                // Socket is there, and is connected. Proceed to make sure MQTT is connected
            }
        }

        self.mqtt_connect(network).map_err(|e| {
            e.map(|e| {
                if matches!(
                    e,
                    EventError::Network(_) | EventError::MqttState(_) | EventError::Timeout
                ) {
                    mqtt_log!(debug, "Disconnecting!");
                    self.disconnect(network);
                }
                e
            })
        })
    }

    fn should_handle_request(&mut self) -> bool {
        let qos_space = self.state.outgoing_pub.len() < self.state.outgoing_pub.capacity();

        // TODO:
        // let qos_0 = if let Some(_) = self.requests.read() {
        //     p.qos == QoS::AtMostOnce
        // } else {
        //     false
        // };

        // qos_0 || (self.requests.ready() && qos_space)
        qos_space
    }

    /// Selects an event from the client's requests, incoming packets from the
    /// broker and keepalive ping cycle.
    fn select_event<N: TcpClientStack<TcpSocket = S> + ?Sized>(
        &mut self,
        network: &mut N,
    ) -> nb::Result<Notification, EventError> {
        let now = self
            .last_outgoing_timer
            .try_now()
            .map_err(EventError::from)?;

        // Handle a request
        if self.should_handle_request() {
            if let Some(mut grant) = self.requests.read() {
                let mut packet = SerializedPacket(grant.deref_mut());
                match self.state.handle_outgoing_request(&mut packet, &now) {
                    Ok(()) => {
                        self.network_handle.send(network, packet.to_inner())?;
                        grant.release();
                        return Err(nb::Error::WouldBlock);
                    }
                    Err(crate::state::StateError::MaxMessagesInflight) => {}
                    Err(e) => return Err(nb::Error::Other(e.into())),
                }
            }
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
            self.network_handle.send_packet(network, &packet)?;
            self.state.last_ping_entry().insert(now);
            return Err(nb::Error::WouldBlock);
        }

        // Handle an incoming packet
        let (notification, packet) = self
            .network_handle
            .receive(network)
            .map_err(|e| e.map(EventError::Network))?
            .decode(&mut self.state)?;

        // Handle `ack` of newly received incoming packet, if relevant
        if let Some(packet) = packet {
            self.network_handle.send_packet(network, &packet)?;
        }

        // By comparing the current time, select pending non-zero QoS publish
        // requests staying longer than the retry interval, and handle their
        // retrial.
        for (pid, inflight) in self.state.retries(now, 10_000.milliseconds()) {
            mqtt_log!(warn, "Retrying PID {:?}", pid);
            // Update inflight's timestamp for later retrials
            inflight.last_touch_entry().insert(now);
            let packet = inflight.packet(*pid).map_err(EventError::from)?;
            self.network_handle.send(network, &packet)?;
        }

        notification.ok_or(nb::Error::WouldBlock)
    }

    /// Yields notification from events. All the error raised while processing
    /// event is reported as an `Ok` value of `Notification::Abort`.
    #[must_use = "Eventloop should be iterated over a loop to make progress"]
    pub fn yield_event<N: TcpClientStack<TcpSocket = S> + ?Sized>(
        &mut self,
        network: &mut N,
    ) -> nb::Result<Notification, Infallible> {
        if self.network_handle.socket.is_none() {
            return Ok(Notification::Abort(EventError::Network(
                NetworkError::NoSocket,
            )));
        }

        self.select_event(network).or_else(|e| match e {
            nb::Error::WouldBlock => Err(nb::Error::WouldBlock),
            nb::Error::Other(e) => {
                mqtt_log!(debug, "Disconnecting from an event error");
                self.disconnect(network);
                Ok(Notification::Abort(e))
            }
        })
    }

    pub fn disconnect<N: TcpClientStack<TcpSocket = S> + ?Sized>(&mut self, network: &mut N) {
        self.state.connection_status = MqttConnectionStatus::Disconnected;
        if let Some(socket) = self.network_handle.socket.take() {
            network.close(socket).ok();
        }
    }

    fn mqtt_connect<N: TcpClientStack<TcpSocket = S> + ?Sized>(
        &mut self,
        network: &mut N,
    ) -> nb::Result<bool, EventError> {
        match self.state.connection_status {
            MqttConnectionStatus::Connected => Ok(false),
            MqttConnectionStatus::Disconnected => {
                mqtt_log!(info, "MQTT connecting..");
                let now = self
                    .last_outgoing_timer
                    .try_now()
                    .map_err(EventError::from)?;
                self.state.last_ping_entry().insert(now);

                self.state.await_pingresp = false;
                self.network_handle.rx_buf.init();

                let (username, password) = self.options.credentials();

                let connect = Packet::Connect(Connect {
                    protocol: Protocol::MQTT311,
                    keep_alive: (self.options.keep_alive_ms() / 1000) as u16,
                    client_id: self.options.client_id(),
                    clean_session: self.options.clean_session(),
                    last_will: self.options.last_will(),
                    username,
                    password,
                });

                // mqtt connection with timeout
                self.network_handle.send_packet(network, &connect)?;
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

                self.network_handle
                    .receive(network)
                    .map_err(|e| e.map(EventError::Network))?
                    .decode(&mut self.state)
                    .and_then(|(n, p)| {
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
    tx_buf: heapless::Vec<u8, 64>,
    rx_buf: PacketBuffer,
}

impl<S> NetworkHandle<S> {
    fn lookup_host<N: Dns + TcpClientStack<TcpSocket = S> + ?Sized>(
        network: &mut N,
        broker: Broker,
        port: u16,
    ) -> Result<(String<256>, SocketAddr), NetworkError> {
        match broker {
            Broker::Hostname(h) => {
                let socket_addr = SocketAddr::new(
                    network.get_host_by_name(h, AddrType::IPv4).map_err(|_e| {
                        mqtt_log!(info, "Failed to resolve IP!");
                        NetworkError::DnsLookupFailed
                    })?,
                    port,
                );
                Ok((String::from(h), socket_addr))
            }
            Broker::IpAddr(ip) => {
                let socket_addr = SocketAddr::new(ip, port);
                let domain = network.get_host_by_address(ip).map_err(|_e| {
                    mqtt_log!(info, "Failed to resolve hostname!");
                    NetworkError::DnsLookupFailed
                })?;

                Ok((domain, socket_addr))
            }
        }
    }

    fn new() -> Self {
        Self {
            socket: None,
            tx_buf: heapless::Vec::new(),
            rx_buf: PacketBuffer::new(),
        }
    }

    /// Checks if this socket is present and connected. Raises `NetworkError` when
    /// the socket is present and in its error state.
    fn is_connected<N: Dns + TcpClientStack<TcpSocket = S> + ?Sized>(
        &self,
        network: &mut N,
    ) -> Result<bool, NetworkError> {
        match self.socket {
            Some(ref socket) => network
                .is_connected(socket)
                .map_err(|_e| NetworkError::SocketClosed),
            None => Err(NetworkError::SocketClosed),
        }
    }

    fn connect<N: Dns + TcpClientStack<TcpSocket = S> + ?Sized>(
        &mut self,
        network: &mut N,
        socket_addr: SocketAddr,
    ) -> Result<(), NetworkError> {
        let socket = match self.socket.as_mut() {
            None => {
                let socket = network.socket().map_err(|_e| NetworkError::SocketOpen)?;
                self.socket.get_or_insert(socket)
            }
            Some(socket) => socket,
        };

        nb::block!(network.connect(socket, socket_addr)).map_err(|_| {
            self.socket.take();
            NetworkError::SocketConnect
        })
    }

    pub fn send_packet<'d, N: TcpClientStack<TcpSocket = S> + ?Sized>(
        &mut self,
        network: &mut N,
        pkt: &Packet,
    ) -> Result<usize, EventError> {
        self.tx_buf.clear();
        self.tx_buf
            .resize_default(self.tx_buf.capacity())
            .unwrap_or_else(|()| unreachable!("Input length equals to the current capacity."));

        let size = encode_slice(&pkt, self.tx_buf.as_mut()).map_err(EventError::Encoding)?;

        let socket = self
            .socket
            .as_mut()
            .ok_or(EventError::Network(NetworkError::NoSocket))?;

        let length = nb::block!(network.send(socket, &self.tx_buf[..size])).map_err(|_| {
            mqtt_log!(error, "[send] NetworkError::Write");
            EventError::Network(NetworkError::Write)
        })?;

        Ok(length)
    }

    pub fn send<'d, N: TcpClientStack<TcpSocket = S> + ?Sized>(
        &mut self,
        network: &mut N,
        pkt: &[u8],
    ) -> Result<usize, EventError> {
        let socket = self
            .socket
            .as_mut()
            .ok_or(EventError::Network(NetworkError::NoSocket))?;

        let length = nb::block!(network.send(socket, &pkt)).map_err(|_| {
            mqtt_log!(error, "[send] NetworkError::Write");
            EventError::Network(NetworkError::Write)
        })?;

        Ok(length)
    }

    fn receive<N: TcpClientStack<TcpSocket = S> + ?Sized>(
        &mut self,
        network: &mut N,
    ) -> nb::Result<PacketDecoder<'_>, NetworkError> {
        let socket = self.socket.as_mut().ok_or(NetworkError::NoSocket)?;

        self.rx_buf.receive(socket, network)?;

        Ok(PacketDecoder::new(&mut self.rx_buf))
    }
}

/// A placeholder that keeps a buffer and constructs a packet incrementally.
/// Given that underlying `TcpClientStack` throws `WouldBlock` in a non-blocking
/// manner, its packet construction won't block either.
#[derive(Debug)]
struct PacketBuffer {
    range: RangeTo<usize>,
    buffer: Vec<u8, 1024>,
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
    fn receive<N, S>(&mut self, socket: &mut S, network: &mut N) -> nb::Result<(), NetworkError>
    where
        N: TcpClientStack<TcpSocket = S> + ?Sized,
    {
        let buffer = self.buffer();
        let len = network.receive(socket, buffer).map_err(|e| {
            if matches!(e, nb::Error::WouldBlock) {
                nb::Error::WouldBlock
            } else {
                mqtt_log!(error, "[receive] NetworkError::Read");
                nb::Error::Other(NetworkError::Read)
            }
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

    fn decode<TIM>(
        mut self,
        state: &mut MqttState<TIM>,
    ) -> nb::Result<(Option<Notification>, Option<Packet<'static>>), EventError>
    where
        TIM: Add<Milliseconds, Output = TIM> + PartialOrd + Copy,
    {
        let buffer = self.packet_buffer.buffer[self.packet_buffer.range].as_ref();
        match decode_slice(buffer) {
            Err(e) => {
                self.is_err.replace(true);
                mqtt_log!(error, "Packet decode error!");

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
    use crate::state::{BoxedPublish, Inflight, StartTime};
    use bbqueue::BBBuffer;
    use embedded_time::clock::Error;
    use embedded_time::duration::Milliseconds;
    use embedded_time::fraction::Fraction;
    use embedded_time::Instant;
    use heapless::pool::singleton::Pool;
    use mqttrust::encoding::v4::{Connack, ConnectReturnCode, Error as EncodingError, Pid};
    use mqttrust::{Publish, QoS};

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

        fn get_host_by_name(
            &mut self,
            _hostname: &str,
            _addr_type: embedded_nal::AddrType,
        ) -> nb::Result<embedded_nal::IpAddr, Self::Error> {
            unimplemented!()
        }
        fn get_host_by_address(
            &mut self,
            _addr: embedded_nal::IpAddr,
        ) -> nb::Result<heapless::String<256>, Self::Error> {
            unimplemented!()
        }
    }

    impl TcpClientStack for MockNetwork {
        type TcpSocket = ();
        type Error = ();

        fn socket(&mut self) -> Result<Self::TcpSocket, Self::Error> {
            Ok(())
        }

        fn connect(
            &mut self,
            _socket: &mut Self::TcpSocket,
            _remote: embedded_nal::SocketAddr,
        ) -> nb::Result<(), Self::Error> {
            Ok(())
        }

        fn is_connected(&mut self, _socket: &Self::TcpSocket) -> Result<bool, Self::Error> {
            Ok(true)
        }

        fn send(
            &mut self,
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
            &mut self,
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

        fn close(&mut self, _socket: Self::TcpSocket) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn success_receive_multiple_packets() {
        let mut state = MqttState::<Milliseconds>::new();
        const LEN: usize = 1024 * 10;
        static mut PUBLISH_MEM: [u8; LEN] = [0u8; LEN];
        BoxedPublish::grow(unsafe { &mut PUBLISH_MEM });

        let mut rx_buf = PacketBuffer::new();
        let connack = Connack {
            session_present: false,
            code: ConnectReturnCode::Accepted,
        };
        let publish = Publish {
            dup: false,
            qos: QoS::AtLeastOnce,
            pid: Some(Pid::new()),
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

        let mut pkg = SerializedPacket(&mut rx_buf.buffer[rx_buf.range]);
        pkg.set_pid(Pid::new()).unwrap();

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
        let mut state = MqttState::<Milliseconds>::new();
        const LEN: usize = 1024 * 10;
        static mut PUBLISH_MEM: [u8; LEN] = [0u8; LEN];
        BoxedPublish::grow(unsafe { &mut PUBLISH_MEM });

        let mut rx_buf = PacketBuffer::new();
        let connack_malformed = Connack {
            session_present: false,
            code: ConnectReturnCode::Accepted,
        };
        let publish = Publish {
            dup: false,
            qos: QoS::AtLeastOnce,
            pid: Some(Pid::new()),
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
                assert_eq!(
                    e,
                    EventError::Encoding(EncodingError::InvalidConnectReturnCode(6))
                )
            }
        }
        assert_eq!(state.connection_status, MqttConnectionStatus::Handshake);
        assert_eq!(rx_buf.range.end, 0);
        assert!((0..1024).all(|i| rx_buf.buffer[i] == 0));
    }

    #[test]
    fn retry_behaviour() {
        static mut Q: BBBuffer<1024> = BBBuffer::new();

        let mut network = MockNetwork {
            should_fail_read: false,
            should_fail_write: false,
        };

        let (_p, c) = unsafe { Q.try_split_framed().unwrap() };
        let mut event = EventLoop::<(), _, 1024>::new(
            c,
            ClockMock { time: 0 },
            MqttOptions::new("client", Broker::Hostname(""), 8883),
        );

        let now = StartTime::from(0);

        let topic = "hello/world";
        let payload = &[1, 2, 3];

        let publish = Publish {
            qos: QoS::AtLeastOnce,
            pid: Some(Pid::new()),
            payload,
            dup: false,
            retain: false,
            topic_name: topic,
        };

        let mut rx_buf = PacketBuffer::new();
        let publish_len = encode_slice(&Packet::from(publish.clone()), rx_buf.buffer()).unwrap();
        rx_buf.range.end += publish_len;

        event
            .state
            .outgoing_pub
            .insert(2, Inflight::new(now, &rx_buf.buffer))
            .unwrap();

        event.state.connection_status = MqttConnectionStatus::Handshake;
        event.network_handle.socket = Some(());

        event.connect(&mut network).unwrap();
    }
}
