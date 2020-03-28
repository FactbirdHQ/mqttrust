#![cfg_attr(not(test), no_std)]

extern crate alloc;
extern crate embedded_nal;
extern crate mqttrs;
#[macro_use]
extern crate nb;

use bytes::BytesMut;
use embedded_nal::TcpStack;
use mqttrs::{decode, encode, ConnectReturnCode, Packet, PacketType, Pid, Publish, QosPid};

pub use mqttrs::{Connect, Protocol, QoS};

use alloc::{string::String, vec::Vec};

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Network,
    Encoding(mqttrs::Error),
    Timeout,
    ConnectionDenied,
    InvalidPacket,
    PongTimeout,
    Busy,
    Unknown,
}

impl From<mqttrs::Error> for Error {
    fn from(e: mqttrs::Error) -> Self {
        Error::Encoding(e)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Idle,
    AwaitingConnack,
    AwaitingPubAck(QoS),
}

pub struct MQTTClient<N, T, P>
where
    N: TcpStack,
{
    pub last_pid: Pid,
    pub keep_alive_ms: u32,
    pub pong_pending: bool,

    pub(crate) state: State,

    pub(crate) rx_buf: BytesMut,
    pub(crate) tx_buf: BytesMut,

    pub(crate) socket: Option<N::TcpSocket>,
    pub(crate) keep_alive_timer: P,
    pub(crate) command_timer: T,
}

impl<N, T, P> MQTTClient<N, T, P>
where
    N: TcpStack,
    T: embedded_hal::timer::CountDown,
    P: embedded_hal::timer::CountDown,
    T::Time: From<u32>,
    P::Time: From<u32>,
{
    pub fn new(keep_alive_timer: P, command_timer: T, rx_size: usize, tx_size: usize) -> Self {
        MQTTClient {
            last_pid: Pid::new(),
            keep_alive_ms: 0,
            pong_pending: false,

            state: State::Idle,

            rx_buf: BytesMut::with_capacity(rx_size),
            tx_buf: BytesMut::with_capacity(tx_size),
            socket: None,
            keep_alive_timer,
            command_timer,
        }
    }

    fn next_pid(&mut self) -> Pid {
        self.last_pid = self.last_pid + 1;
        self.last_pid
    }

    fn send_buffer(&mut self, network: &N) -> Result<(), Error> {
        if let Some(ref mut socket) = self.socket {
            block!(network.write(socket, &self.tx_buf)).map_err(|_| Error::Network)?;
            self.tx_buf.clear();

            // reset keep alive timer
            self.keep_alive_timer.start(self.keep_alive_ms);
            Ok(())
        } else {
            Err(Error::Network)
        }
    }

    fn cycle(&mut self, network: &N) -> nb::Result<Packet, Error> {
        if let Some(ref mut socket) = self.socket {
            // TODO: It seems silly to create an additional buffer here, only to copy more
            let mut buf = [0u8; 256];
            let size = network
                .read(socket, &mut buf)
                .map_err(|_e| Error::Network)?;

            if size == 0 {
                // No data available
                return Err(nb::Error::WouldBlock);
            }

            self.rx_buf.extend_from_slice(&buf[0..size]);

            let packet = decode(&mut self.rx_buf)
                .map_err(|e| Error::Encoding(e))?
                .ok_or(nb::Error::WouldBlock)?;

            match &packet {
                Packet::Publish(p) => {
                    // Handle callbacks!

                    match p.qospid {
                        QosPid::AtMostOnce => {}
                        QosPid::AtLeastOnce(pid) => {
                            let pkt = Packet::Puback(pid).into();
                            encode(&pkt, &mut self.tx_buf).map_err(|e| Error::Encoding(e))?;
                            self.send_buffer(network)?;
                        }
                        QosPid::ExactlyOnce(pid) => {
                            let pkt = Packet::Pubrec(pid).into();
                            encode(&pkt, &mut self.tx_buf).map_err(|e| Error::Encoding(e))?;
                            self.send_buffer(network)?;
                        }
                    }
                }
                Packet::Pubrec(pid) => {
                    let pkt = Packet::Pubrel(pid.clone()).into();
                    encode(&pkt, &mut self.tx_buf).map_err(|e| Error::Encoding(e))?;
                    self.send_buffer(network)?;
                }
                Packet::Pubrel(pid) => {
                    let pkt = Packet::Pubcomp(pid.clone()).into();
                    encode(&pkt, &mut self.tx_buf).map_err(|e| Error::Encoding(e))?;
                    self.send_buffer(network)?;
                }
                Packet::Pingresp => {
                    self.pong_pending = false;
                }
                _ => {}
            };

            Ok(packet)
        } else {
            Err(nb::Error::Other(Error::Network))
        }
    }

    fn cycle_until(&mut self, network: &N, packet_type: PacketType) -> nb::Result<Packet, Error> {
        let packet = self.cycle(network)?;
        if packet.get_type() == packet_type {
            Ok(packet)
        } else {
            Err(nb::Error::Other(Error::InvalidPacket))
        }
    }

    pub fn publish(
        &mut self,
        network: &N,
        qos: QoS,
        topic_name: String,
        payload: Vec<u8>,
    ) -> nb::Result<(), Error> {
        Err(match self.state {
            State::Idle => {
                let pid = self.next_pid();

                let qospid = match qos {
                    QoS::AtMostOnce => QosPid::AtMostOnce,
                    QoS::AtLeastOnce => QosPid::AtLeastOnce(pid),
                    QoS::ExactlyOnce => QosPid::ExactlyOnce(pid),
                };

                let pkt = Publish {
                    dup: false,
                    qospid,
                    retain: false,
                    topic_name,
                    payload,
                };
                encode(&pkt.into(), &mut self.tx_buf).map_err(|e| nb::Error::Other(e.into()))?;
                self.send_buffer(network)?;
                self.state = State::AwaitingPubAck(qospid.qos());
                self.command_timer.start(5000);
                nb::Error::WouldBlock
            }
            State::AwaitingPubAck(qos) => {
                if self.command_timer.wait().is_ok() {
                    self.state = State::Idle;
                    return Err(nb::Error::Other(Error::Timeout));
                }

                let error = match qos {
                    QoS::AtMostOnce => return Ok(()),
                    QoS::AtLeastOnce => match self.cycle_until(network, PacketType::Puback) {
                        Ok(_) => {
                            self.state = State::Idle;
                            return Ok(());
                        }
                        Err(e) => e,
                    },
                    QoS::ExactlyOnce => match self.cycle_until(network, PacketType::Pubcomp) {
                        Ok(_) => {
                            self.state = State::Idle;
                            return Ok(());
                        }
                        Err(e) => e,
                    },
                };
                if let nb::Error::Other(_) = error {
                    self.state = State::Idle;
                };
                error
            }
            _ => nb::Error::Other(Error::Busy),
        })
    }

    pub fn yield_client(&mut self) -> Result<(), Error> {
        // self.cycle_until()
        match self.state {
            State::AwaitingConnack => {}
            _ => {}
        }
        Ok(())
    }

    pub fn keep_alive(&mut self, network: &N) -> Result<(), Error> {
        // return immediately if keep alive interval is zero
        if self.keep_alive_ms == 0 {
            return Ok(());
        };

        // return immediately if no ping is due
        if self.keep_alive_timer.wait().is_ok() {
            return Ok(());
        }

        // a ping is due

        // fail immediately if a pong is already pending
        if self.pong_pending {
            return Err(Error::PongTimeout);
        };

        // encode & send pingreq packet
        let pkt = Packet::Pingreq.into();
        encode(&pkt, &mut self.tx_buf)?;
        self.send_buffer(network)?;

        // set flag
        self.pong_pending = true;

        Ok(())
    }

    pub fn disconnect(&mut self, network: &N) -> Result<(), Error> {
        // encode & send disconnect packet
        let pkt = Packet::Disconnect.into();
        encode(&pkt, &mut self.tx_buf)?;
        self.send_buffer(network)?;
        Ok(())
    }

    pub fn connect(
        &mut self,
        network: &N,
        socket: N::TcpSocket,
        options: Connect,
    ) -> nb::Result<(), Error> {
        Err(match self.state {
            State::Idle => {
                self.socket = Some(socket);

                // save keep alive interval
                self.keep_alive_ms = (options.keep_alive as u32) * 1000;

                // set keep alive timer
                self.keep_alive_timer.start(self.keep_alive_ms);

                // reset pong pending flag
                self.pong_pending = false;

                // encode connect packet
                encode(&options.into(), &mut self.tx_buf)
                    .map_err(|e| nb::Error::Other(e.into()))?;

                // send packet
                self.send_buffer(network)?;

                // wait for connack packet
                self.command_timer.start(5000);
                self.state = State::AwaitingConnack;

                nb::Error::WouldBlock
            }
            State::AwaitingConnack => {
                if self.command_timer.wait().is_ok() {
                    self.state = State::Idle;
                    return Err(nb::Error::Other(Error::Timeout));
                }

                match self.cycle_until(network, PacketType::Connack) {
                    Ok(Packet::Connack(c)) => {
                        self.state = State::Idle;
                        if c.code != ConnectReturnCode::Accepted {
                            nb::Error::Other(Error::ConnectionDenied)
                        } else {
                            return Ok(());
                        }
                    }
                    Ok(_) => {
                        self.state = State::Idle;
                        nb::Error::Other(Error::InvalidPacket)
                    }
                    Err(e) => {
                        match e {
                            nb::Error::Other(_) => {
                                self.state = State::Idle;
                            }
                            _ => {}
                        };
                        e
                    }
                }
            }
            _ => nb::Error::Other(Error::Busy),
        })
    }
}
