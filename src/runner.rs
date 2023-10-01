use core::{
    convert::Infallible,
    ops::{Deref, DerefMut},
};

use bbqueue::{framed::FrameProducer, BufferProvider, SliceBufferProvider};
use futures::{
    future::{select, Either},
    pin_mut,
};
use heapless::{String, Vec};

use crate::{
    de::{packet_header::PacketHeader, received_packet::ReceivedPacket, PacketReader},
    error::{Error, ProtocolError},
    message_types::MessageType,
    QoS,
};

const MAX_TOPIC_LEN: usize = 128;
const MAX_SUBSCRIPTIONS: usize = 5;

type Topic = String<MAX_TOPIC_LEN>;

struct MPSCChannel;
impl MPSCChannel {
    fn new() -> Self {
        Self
    }

    async fn receive(&mut self) -> ReceivedPacket<'_> {
        todo!()
    }
}

struct Network;
impl embedded_io_async::ErrorType for Network {
    type Error = Infallible;
}

impl embedded_io_async::Write for Network {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        todo!()
    }
}

impl embedded_io_async::Read for Network {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        todo!()
    }
}

struct Runner<const MAX_CLIENTS: usize> {
    clients: [Option<(
        Vec<Topic, MAX_SUBSCRIPTIONS>,
        FrameProducer<'static, SliceBufferProvider<'static>>,
    )>; MAX_CLIENTS],
    outgoing: MPSCChannel,
    network: Network,
}

impl<const MAX_CLIENTS: usize> Runner<MAX_CLIENTS> {
    pub fn new() -> Self {
        const INIT_SUB: Option<(
            Vec<Topic, MAX_SUBSCRIPTIONS>,
            FrameProducer<'static, SliceBufferProvider<'static>>,
        )> = None;

        Self {
            clients: [INIT_SUB; MAX_CLIENTS],
            outgoing: MPSCChannel::new(),
            network: Network,
        }
    }

    fn next_client_id(&self) -> Result<u8, Error> {
        for (i, c) in self.clients.iter().enumerate() {
            if c.is_none() {
                return Ok(i as u8);
            }
        }
        Err(Error::TooManyClients)
    }

    pub fn client<'rx>(
        &mut self,
        rx_chan: &'static mut bbqueue::BBQueue<SliceBufferProvider<'rx>>,
    ) -> Result<Client<'rx>, Error> {
        let (producer, consumer) = rx_chan.try_split_framed().unwrap();

        let id = self.next_client_id()?;
        self.clients[id as usize].replace((Vec::new(), producer));

        Ok(Client {
            id,
            rx: MqttConsumer(consumer),
        })
    }

    pub async fn run(&mut self) -> ! {
        let mut reader = PacketReader::new();

        loop {
            // DO STUFF!
            // Remember to:
            // - handle `max inflight messages`, Optionally allow QoS0 to be transmitted always
            // - handle keep-alive ping/pong
            // - allow room for persistance spooling

            let next_packet_fut = reader.next_packet_header(&mut self.network);
            let receive_fut = self.outgoing.receive();
            pin_mut!(next_packet_fut);
            pin_mut!(receive_fut);

            match select(next_packet_fut, receive_fut).await {
                Either::Left((packet, _)) => {
                    Self::handle_incoming(&mut self.clients, packet.unwrap()).await
                }
                Either::Right((packet, _)) => Self::handle_outgoing().await,
            };
        }
    }

    async fn handle_incoming(
        clients: &mut [Option<(
            Vec<Topic, MAX_SUBSCRIPTIONS>,
            FrameProducer<'static, SliceBufferProvider<'static>>,
        )>],
        packet: PacketHeader<'_>,
    ) {
        match packet.message_type() {
            MessageType::Publish => {
                // Handle QoS ack response
                match packet.qos().unwrap() {
                    QoS::AtMostOnce => {}
                    QoS::AtLeastOnce => {
                        let packet_id = packet.pid().unwrap();

                        // let reason = if self.state.server_packet_id_in_use(packet_id) {
                        //     ReasonCode::PacketIdInUse
                        // } else {
                        //     ReasonCode::Success
                        // };

                        // let puback = PubAck {
                        //     packet_identifier: packet_id,
                        //     reason: reason.into(),
                        // };

                        // self.network.write_all(&puback).await;
                    }
                    QoS::ExactlyOnce => {}
                }

                // Notify subscribed client
                for (ref topics, ref mut writer) in clients.iter_mut().filter_map(|s| s.as_mut()) {
                    if topics.iter().any(|t| t.as_str() == packet.topic().unwrap()) {
                        let mut grant = writer.grant_async(packet.len()).await.unwrap();
                        let len = packet
                            .copy_packet(grant.deref_mut(), &mut Network)
                            .await
                            .unwrap();
                        grant.commit(len);
                    }
                }
            }
            MessageType::PubAck => {
                // let pub_id = self.pid_to_publisher(pid)?;

                // Handle QoS2 if supported?

                // if let Some((_, Some(waker))) = self
                //     .publishers
                //     .iter_mut()
                //     .enumerate()
                //     .find(|(id, _)| id == pub_id)
                // {
                //     waker.take().wake();
                // }
            }
            _ => {}
        }
    }

    async fn handle_outgoing() {

        // match packet.type() {
        //     PacketType::Subscribe => {

        //     }
        // }
    }
}

pub struct Client<'buf> {
    id: u8,
    rx: MqttConsumer<'buf>,
}

struct MqttConsumer<'a>(bbqueue::framed::FrameConsumer<'static, SliceBufferProvider<'a>>);

impl<'a> MqttConsumer<'a> {
    pub async fn next_packet(&mut self) -> Result<ReceivedPacket<'a>, ProtocolError> {
        let mut r = self
            .0
            .read_async()
            .await
            .map_err(|_| ProtocolError::BufferSize)?;
        r.auto_release(true);
        ReceivedPacket::from_buffer(unsafe { core::mem::transmute::<&[u8], &'static [u8]>(&r) })
    }
}
