use crate::requests::{PublishPayload, Request};
use crate::state::{MqttConnectionStatus, MqttState};
use crate::MqttOptions;
use crate::{EventError, NetworkError, Notification};
use core::convert::{AsMut, TryInto};
use core::ops::RangeTo;
use embedded_nal::TcpClient;
use heapless::{consts, spsc, ArrayLength, Vec};
use mqttrs::{decode_slice, encode_slice, Connect, Packet, Protocol, QoS};

/// Encapsulate application-layer transaction. For example, an implementer can
/// be a raw TCP traffic, TLS or WebSocket. Typically `T` implements `TcpClient`
/// and `Self` would be a newtype wrapping `TcpClient::TcpSocket`. The
/// underlying socket must be connected to a broker in advance and ready for
/// read/write, e.g., having done SSL/TLS handshake if it is a TLS socket. It is
/// also a user's responsibility to disconnect it on abort notification. A
/// buffer is supposed to contain contiguous MQTT payloads encoded without
/// encryption.
pub trait Session<T> {
    /// An error representing transaction failures.
    type Error;
    /// Non-blocking read from the underlying socket.
    fn try_read(&mut self, stack: &T, buffer: &mut [u8]) -> nb::Result<usize, Self::Error>;
    /// Non-blocking write to the underlying socket.
    fn try_write(&mut self, stack: &T, buffer: &[u8]) -> nb::Result<usize, Self::Error>;
}

/// A newtype wrapping a socket of type `S`, given another type `T` of
/// `TcpClient<TcpSocket = S>`.
pub struct TcpSession<S>(S);

impl<S> TcpSession<S> {
    /// Releases the underlying socket.
    pub fn free(self) -> S {
        self.0
    }
}

impl<S> From<S> for TcpSession<S> {
    fn from(socket: S) -> Self {
        Self(socket)
    }
}

impl<T, S> Session<T> for TcpSession<S>
where
    T: TcpClient<TcpSocket = S>,
{
    type Error = <T as TcpClient>::Error;
    fn try_read(&mut self, stack: &T, buffer: &mut [u8]) -> nb::Result<usize, Self::Error> {
        stack.receive(&mut self.0, buffer)
    }

    fn try_write(&mut self, stack: &T, buffer: &[u8]) -> nb::Result<usize, Self::Error> {
        stack.send(&mut self.0, buffer)
    }
}

pub struct EventLoop<'a, 'b, L, O, P>
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
    /// Request stream
    pub requests: spsc::Consumer<'a, Request<P>, L, u8>,
    tx_buf: Vec<u8, consts::U1024>,
    rx_buf: PacketBuffer,
}

impl<'a, 'b, L, O, P> EventLoop<'a, 'b, L, O, P>
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
            requests,
            tx_buf: Vec::new(),
            rx_buf: PacketBuffer::new(),
        }
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

    pub fn yield_event<T, S>(&mut self, stack: &T, session: &mut S) -> nb::Result<Notification, ()>
    where
        S: Session<T>,
    {
        let packet_buf = &mut [0u8; 1024];
        let o = if self.should_handle_request() {
            // Handle requests
            let request = unsafe { self.requests.dequeue_unchecked() };
            self.state
                .handle_outgoing_request(request, packet_buf)
                .map_err(EventError::from)
        } else if self.last_outgoing_timer.try_wait().is_ok() {
            // Handle ping
            self.state
                .handle_outgoing_packet(Packet::Pingreq)
                .map_err(EventError::from)
        } else {
            self.receive(stack, session)
        };

        let (notification, outpacket) = match o {
            Ok((n, p)) => (n, p),
            Err(e) => {
                defmt::debug!("Got an error while handling the incoming packet.");
                return Ok(Notification::Abort(e));
            }
        };

        if let Some(p) = outpacket {
            if let Err(e) = self.send(stack, session, p) {
                defmt::debug!("Failed to send an outgoing packet.");
                return Ok(Notification::Abort(e));
            } else {
                self.last_outgoing_timer
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

    pub fn send<'d, T, S>(
        &mut self,
        stack: &T,
        session: &mut S,
        pkt: Packet<'d>,
    ) -> Result<usize, EventError>
    where
        S: Session<T>,
    {
        let capacity = self.tx_buf.capacity();
        self.tx_buf.clear();
        self.tx_buf
            .resize(capacity, 0x00u8)
            .unwrap_or_else(|()| unreachable!("Input length equals to the current capacity."));
        let size = encode_slice(&pkt, self.tx_buf.as_mut())?;
        nb::block!(session.try_write(stack, &self.tx_buf[..size])).map_err(|_| {
            defmt::error!("[send] NetworkError::Write");
            EventError::Network(NetworkError::Write)
        })
    }

    pub fn receive<T, S>(
        &mut self,
        stack: &T,
        session: &mut S,
    ) -> Result<(Option<Notification>, Option<Packet<'static>>), EventError>
    where
        S: Session<T>,
    {
        match self.rx_buf.receive(stack, session) {
            Err(nb::Error::WouldBlock) => return Ok((None, None)),
            Err(_) => return Err(EventError::Network(NetworkError::Read)),
            _ => {}
        }

        PacketDecoder::new(&mut self.state, &mut self.rx_buf).try_into()
    }

    pub fn connect<T, S>(&mut self, stack: &T, session: &mut S) -> nb::Result<bool, EventError>
    where
        S: Session<T>,
    {
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
                match self.send(stack, session, connect.into()) {
                    Ok(_) => {
                        self.state
                            .handle_outgoing_connect()
                            .map_err(|e| nb::Error::Other(e.into()))?;

                        self.last_outgoing_timer.try_start(50000).ok();
                    }
                    Err(e) => {
                        defmt::debug!("Failed to send a connect packet.");
                        return Err(nb::Error::Other(e));
                    }
                }

                Err(nb::Error::WouldBlock)
            }
            MqttConnectionStatus::Handshake => {
                if self.last_outgoing_timer.try_wait().is_ok() {
                    defmt::debug!("Handshake timed out");
                    return Err(nb::Error::Other(EventError::Timeout));
                }

                self.receive(stack, session)
                    .map_err(|e| nb::Error::Other(e.into()))
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
    fn receive<T, S>(&mut self, stack: &T, session: &mut S) -> nb::Result<(), EventError>
    where
        S: Session<T>,
    {
        let buffer = self.buffer();
        let len = session
            .try_read(stack, buffer)
            .map_err(|e| e.map(|_| EventError::Network(NetworkError::Read)))?;
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
    type Error = EventError;
    fn try_into(mut self) -> Result<(Option<Notification>, Option<Packet<'static>>), Self::Error> {
        let buffer = self.packet_buffer.buffer[self.packet_buffer.range].as_ref();
        match decode_slice(buffer) {
            Err(_e) => {
                self.is_err.replace(true);
                Err(EventError::Network(NetworkError::Read))
            }
            Ok(Some(packet)) => {
                self.is_err.replace(false);
                self.state
                    .handle_incoming_packet(packet)
                    .map_err(EventError::from)
            }
            Ok(None) => Ok((None, None)),
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

    struct MockSession {
        pub should_fail_read: bool,
        pub should_fail_write: bool,
    }

    impl Session<()> for MockSession {
        type Error = ();

        fn try_write(&mut self, _stack: &(), buffer: &[u8]) -> nb::Result<usize, Self::Error> {
            if self.should_fail_write {
                Err(nb::Error::Other(()))
            } else {
                Ok(buffer.len())
            }
        }

        fn try_read(&mut self, _stack: &(), buffer: &mut [u8]) -> nb::Result<usize, Self::Error> {
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
            Ok((_, _)) => panic!(),
            Err(e) => {
                assert_eq!(e, EventError::Network(NetworkError::Read))
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

        let mut session = MockSession {
            should_fail_read: false,
            should_fail_write: false,
        };

        let (_p, c) = unsafe { Q.split() };
        let mut event =
            EventLoop::<_, _, _>::new(c, CdMock { time: 0 }, MqttOptions::new("client"));

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
        event.connect(&(), &mut session).unwrap();
    }
}
