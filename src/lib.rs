#![cfg_attr(not(test), no_std)]

extern crate alloc;
extern crate embedded_nal;
extern crate mqttrs;
#[macro_use]
extern crate nb;

use embedded_timeout_macros::{block_timeout, TimeoutError};

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
    Unknown,
}

impl<E> From<TimeoutError<E>> for Error {
    fn from(_e: TimeoutError<E>) -> Self {
        Error::Timeout
    }
}

impl From<mqttrs::Error> for Error {
    fn from(e: mqttrs::Error) -> Self {
        Error::Encoding(e)
    }
}

pub struct MQTTClient<'a, N, T>
where
    N: TcpStack,
{
    pub last_pid: Pid,
    pub keep_alive_interval: u32,
    pub pong_pending: bool,

    pub(crate) rx_buf: BytesMut,
    pub(crate) tx_buf: BytesMut,

    pub(crate) network: &'a N,
    pub(crate) socket: N::TcpSocket,
    pub(crate) keep_alive_timer: T,
    pub(crate) command_timer: T,
}

impl<'a, N, T> MQTTClient<'a, N, T>
where
    N: TcpStack,
    T: embedded_hal::timer::CountDown,
    T::Time: From<u32>,
{
    pub fn new(
        network: &'a N,
        socket: N::TcpSocket,
        keep_alive_timer: T,
        command_timer: T,
        rx_size: usize,
        tx_size: usize,
    ) -> Self {
        MQTTClient {
            last_pid: Pid::new(),
            keep_alive_interval: 0,
            pong_pending: false,

            rx_buf: BytesMut::with_capacity(rx_size),
            tx_buf: BytesMut::with_capacity(tx_size),
            network,
            socket,
            keep_alive_timer,
            command_timer,
        }
    }

    fn next_pid(&mut self) -> Pid {
        self.last_pid = self.last_pid + 1;
        self.last_pid
    }

    fn send_buffer(&mut self) -> Result<(), Error> {
        block!(self.network.write(&mut self.socket, &self.tx_buf)).map_err(|_| Error::Network)?;

        // reset keep alive timer
        self.keep_alive_timer.start(self.keep_alive_interval);
        Ok(())
    }

    fn cycle(&mut self) -> nb::Result<Packet, Error> {
        let size = self
            .network
            .read(&mut self.socket, &mut self.rx_buf)
            .map_err(|_| Error::Network)?;

        if size == 0 {
            // No data available
            return Err(nb::Error::WouldBlock);
        }

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
                        self.send_buffer()?;
                    }
                    QosPid::ExactlyOnce(pid) => {
                        let pkt = Packet::Pubrec(pid).into();
                        encode(&pkt, &mut self.tx_buf).map_err(|e| Error::Encoding(e))?;
                        self.send_buffer()?;
                    }
                }
            }
            Packet::Pubrec(pid) => {
                let pkt = Packet::Pubrel(pid.clone()).into();
                encode(&pkt, &mut self.tx_buf).map_err(|e| Error::Encoding(e))?;
                self.send_buffer()?;
            }
            Packet::Pubrel(pid) => {
                let pkt = Packet::Pubcomp(pid.clone()).into();
                encode(&pkt, &mut self.tx_buf).map_err(|e| Error::Encoding(e))?;
                self.send_buffer()?;
            }
            Packet::Pingresp => {
                self.pong_pending = false;
            }
            _ => {}
        };

        Ok(packet)
    }

    fn cycle_until(&mut self, packet_type: PacketType) -> nb::Result<Packet, Error> {
        let packet = self.cycle()?;
        if packet.get_type() == packet_type {
            Ok(packet)
        } else {
            Err(nb::Error::Other(Error::InvalidPacket))
        }
    }

    pub fn publish(&mut self, qos: QoS, topic_name: String, payload: Vec<u8>) -> Result<(), Error> {
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
        encode(&pkt.into(), &mut self.tx_buf)?;
        self.send_buffer()?;

        match qospid.qos() {
            QoS::AtMostOnce => {}
            QoS::AtLeastOnce => {
                block!(self.cycle_until(PacketType::Puback))?;
            }
            QoS::ExactlyOnce => {
                block!(self.cycle_until(PacketType::Pubcomp))?;
            }
        };

        Ok(())
    }

    pub fn yield_client(&mut self) -> Result<(), Error> {
        // self.cycle_until()
        Ok(())
    }

    pub fn keep_alive(&mut self) -> Result<(), Error> {
        // return immediately if keep alive interval is zero
        if self.keep_alive_interval == 0 {
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
        self.send_buffer()?;

        // set flag
        self.pong_pending = true;

        Ok(())
    }

    pub fn disconnect(&mut self) -> Result<(), Error> {
        // encode & send disconnect packet
        let pkt = Packet::Disconnect.into();
        encode(&pkt, &mut self.tx_buf)?;
        self.send_buffer()?;
        Ok(())
    }

    pub fn connect(&mut self, options: Connect) -> Result<(), Error> {
        // save keep alive interval
        self.keep_alive_interval = options.keep_alive as u32;

        // set keep alive timer
        self.keep_alive_timer.start(self.keep_alive_interval);

        // reset pong pending flag
        self.pong_pending = false;

        // encode connect packet
        encode(&options.into(), &mut self.tx_buf)?;

        // send packet
        self.send_buffer()?;

        // wait for connack packet
        if let Packet::Connack(c) = block_timeout!(
            &mut self.command_timer,
            self.cycle_until(PacketType::Connack)
        )? {
            if c.code != ConnectReturnCode::Accepted {
                Err(Error::ConnectionDenied)
            } else {
                Ok(())
            }
        } else {
            Err(Error::InvalidPacket)
        }
    }
}
