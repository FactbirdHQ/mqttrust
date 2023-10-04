use core::{
    cell::RefCell,
    future::poll_fn,
    mem::MaybeUninit,
    sync::atomic::{AtomicU16, Ordering},
    task::Poll,
};

use bbqueue::{
    framed::{FrameConsumer, FrameProducer},
    BBQueue, SliceBufferProvider,
};
use embassy_futures::select::{select3, Either3};
use embassy_sync::{
    blocking_mutex::raw::RawMutex, mutex::Mutex, pubsub::{Publisher, PubSubChannel, Subscriber}, waitqueue::WakerRegistration,
};
use serde::Serialize;

use crate::{
    de::packet_header::PacketHeader, message_types::ControlPacket, packets::{Pub, Subscribe},
    publication::ToPayload, ring_buffer::RingBuffer,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Todo,
}

struct MqttBuffer<'a>(RingBuffer<'a>);

impl<'a> MqttBuffer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self(RingBuffer::new(buf))
    }

    pub(crate) fn write_packet<P: serde::Serialize + ControlPacket>(
        &mut self,
        packet: &P,
    ) -> Result<usize, Error> {
        let buf = self.0.push_buf();
        let (offset, message) = crate::ser::MqttSerializer::to_buffer_meta(buf, packet).unwrap();
        let len = offset + message.len();
        self.0.push(len);
        Ok(len)
    }

    pub(crate) fn packet(&self) -> Option<PacketHeader<'_>> {
        PacketHeader::from_buffer(&self.0.buf()).ok()
    }
}

pub struct Pid(AtomicU16);

impl Pid {
    pub fn new() -> Self {
        Self(AtomicU16::new(0))
    }

    pub fn next(&mut self) -> u16 {
        self.0
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |prev| {
                if prev == u16::MAX {
                    Some(1)
                } else {
                    Some(prev + 1)
                }
            })
            .unwrap()
    }
}

const MAX_SUBS: usize = 3;

pub struct State<M: RawMutex, const TX: usize, const RX: usize> {
    tx: [u8; TX],
    rx: [u8; RX],
    inner: MaybeUninit<StateInner<'static, M>>,
}

impl<M: RawMutex, const TX: usize, const RX: usize> State<M, TX, RX> {
    pub const fn new() -> Self {
        Self {
            tx: [0; TX],
            rx: [0; RX],
            inner: MaybeUninit::uninit(),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum ConnectionState {
    Disconnected,
    Connected,
}

struct StateInner<'a, M: RawMutex> {
    tx: BBQueue<SliceBufferProvider<'a>>,
    subscriptions: PubSubChannel<M, u8, 10, MAX_SUBS, 1>,
    shared: Mutex<M, RefCell<Shared>>,
}

pub struct Shared {
    next_pid: Pid,

    conn_state: ConnectionState,
    tx_waker: WakerRegistration,
}

pub struct MqttStack<'a, M: RawMutex> {
    shared: &'a Mutex<M, RefCell<Shared>>,
    tx_consumer: RefCell<FrameConsumer<'a, SliceBufferProvider<'a>>>,
    rx_producer: RefCell<Publisher<'a, M, u8, 10, MAX_SUBS, 1>>,
    // MqttStack should hold an RX buffer just big enough to hold a single
    // packet header + `MAX_TOPIC_LEN`, in order for it to handle `ACK`,
    // `PING`, and route incoming `PUBLISH` messages to a subscriber.
    rx: [u8; 128],
    // Network handle
    // MqttState
    // MqttConfig
}

impl<'a, M: RawMutex> MqttStack<'a, M> {
    pub async fn run(&self) -> ! {
        loop {
            let mut tx_consumer = self.tx_consumer.borrow_mut();

            let rx_fut = async {
                // self.network.read(&mut self.rx).await
            };

            let tx_fut = tx_consumer.read_async();

            let ping_fut = async {};

            match select3(rx_fut, tx_fut, ping_fut).await {
                Either3::First(_) => {
                    // ### RX future:
                    //
                    // Handle to all incoming packet types by sending ack & waking
                    // `tx_wakers`.
                    //
                    // Handle incoming `Publish` type packets by copying the full
                    // packet to any `shared.subscriptions` buffers where topic
                    // matches their topic_filter.
                }
                Either3::Second(Ok(tx_grant)) => {
                    // ### TX future:
                    // Based on packet QoS, add PID to state & full packet to
                    // retry buffer

                    // let mut packet = SerializedPacket::new(tx_grant.deref_mut())?;
                    // match packet.qos() {
                    //     QoS::AtMostOnce => {}
                    //     QoS::AtLeastOnce => {}
                    //     QoS::ExactlyOnce => {}
                    // }
                    // self.network.write(packet.inner()).await?;
                    tx_grant.release();
                }
                Either3::Third(_) => {
                    // ### PING future:
                    // Timer set to expire based on self.config.keep_alive, and reset on every RX or TX
                }
                _ => {}
            }
        }
    }
}

pub struct MqttClient<'a, M: RawMutex> {
    tx_producer: Mutex<M, RefCell<FrameProducer<'a, SliceBufferProvider<'a>>>>,
    shared: &'a Mutex<M, RefCell<Shared>>,
    channel: &'a PubSubChannel<M, u8, 10, MAX_SUBS, 1>
}

impl<'a, M: RawMutex> MqttClient<'a, M> {
    async fn send(&self, packet: impl Serialize + ControlPacket) -> Result<u16, Error> {
        let pid = self.shared.lock().await.borrow_mut().next_pid.next();

        // Push the serialized packet into `tx_producer`
        let tx_prod_ref = self.tx_producer.lock().await;
        let mut tx_prod = tx_prod_ref.borrow_mut();
        // let max_size = packet.len();
        let max_size = 0;
        let mut grant = tx_prod.grant(max_size).map_err(|_| Error::Todo)?;
        // let actual_len = encode_slice(&packet, grant.deref_mut())?;
        let actual_len = 0;
        grant.commit(actual_len);

        // Wait until `pid` has been ack'd (known by it being removed from the state)
        // FIXME: How to know if it's an ACK or a NACK?
        poll_fn(|cx| {
            if let Ok(shared) = self.shared.try_lock() {
                let mut state = shared.borrow_mut();

                // TODO: Change this to check if state still contains
                if state.conn_state == ConnectionState::Connected {
                    return Poll::Ready(());
                }

                state.tx_waker.register(cx.waker());
                return Poll::Pending;
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        })
        .await;

        Ok(pid)
    }

    pub async fn publish<P: ToPayload>(&self, packet: impl Into<Pub<'_, P>>) -> Result<(), Error> {
        // self.send(packet.into()).await;

        Ok(())
    }

    pub async fn subscribe(
        &self,
        packet: impl Into<Subscribe<'_>>,
    ) -> Result<Subscription<'a, M>, Error> {
        let subscriber = self.channel.subscriber().map_err(|_| Error::Todo)?;

        self.send(packet.into()).await?;

        Ok(Subscription {
            subscriber
        })
    }
}

pub struct Subscription<'a, M: RawMutex> {
    subscriber: Subscriber<'a, M, u8, 10, MAX_SUBS, 1>,
}

impl<'a, M: RawMutex> futures::Stream for Subscription<'a, M> {
    type Item = Message;

    fn poll_next(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match self.subscriber.try_next_message_pure() {
            Some(msg) => {
                Poll::Ready(Some(Message(msg)))
            },
            None => {
                cx.waker().wake_by_ref();
                Poll::Pending
            },
        }
    }
}

pub struct Message(u8);

// impl<'a> Message<'a> {
//     pub fn topic(&self) -> &str {
//         core::str::from_utf8(&[*self.0]).unwrap()
//     }

//     pub fn payload(&self) -> &[u8] {
//         &[*self.0]
//     }
// }

// impl<'a, M: RawMutex> Drop for Subscription<'a, M> {
//     fn drop(&mut self) {
//         self.shared
//             .try_lock()
//             .unwrap()
//             .borrow_mut()
//             .unregister_subscription(self.handle);
//     }
// }

pub fn new<'d, M: RawMutex, const TX: usize, const RX: usize>(
    state: &'d mut State<M, TX, RX>,
) -> (MqttStack<'d, M>, MqttClient<'d, M>) {
    // safety: this is a self-referential struct, however:
    // - it can't move while the `'d` borrow is active.
    // - when the borrow ends, the dangling references inside the MaybeUninit will never be used again.
    let state_uninit: *mut MaybeUninit<StateInner<'d, M>> =
        (&mut state.inner as *mut MaybeUninit<StateInner<'static, M>>).cast();

    let state = unsafe { &mut *state_uninit }.write(StateInner {
        tx: BBQueue::new(SliceBufferProvider::new(&mut state.tx[..])),
        subscriptions: PubSubChannel::new(),
        shared: Mutex::new(RefCell::new(Shared {
            next_pid: Pid::new(),

            conn_state: ConnectionState::Disconnected,
            tx_waker: WakerRegistration::new(),
        })),
    });

    let (tx_producer, tx_consumer) = state.tx.try_split_framed().unwrap();

    (
        MqttStack {
            tx_consumer: RefCell::new(tx_consumer),
            rx_producer: RefCell::new(state.subscriptions.publisher().unwrap()),
            shared: &state.shared,
            rx: [0; 128],
        },
        MqttClient {
            tx_producer: Mutex::new(RefCell::new(tx_producer)),
            shared: &state.shared,
            channel: &state.subscriptions
        },
    )
}

#[cfg(test)]
mod tests {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use futures::StreamExt;

    use crate::{publication::Publication, types::Properties};

    use super::*;

    #[tokio::test]
    async fn subscribe_publish() {
        // Create the MQTT stack
        let state = static_cell::make_static!(State::<NoopRawMutex, 4096, 4096>::new());
        let (stack, client) = super::new(state);

        // Use the MQTT client to subscribe
        let subscribe = crate::packets::Subscribe {
            packet_id: 16,
            properties: Properties::Slice(&[]),
            topics: &["ABC".into()],
        };

        let mut rx_a = [0u8; 512];
        let queue = bbqueue::BBQueue::new_from_slice(&mut rx_a);
        // let mut subscription = client.subscribe(&queue, subscribe).await.unwrap();

        // client
        //     .publish(Publication::new(b"").topic("ABC").finish().unwrap())
        //     .await
        //     .unwrap();

        // while let Some(message) = subscription.next().await {
        //     match message.topic() {
        //         "ABC" => {
        //             client
        //                 .publish(Publication::new(b"").topic("ABC").finish().unwrap())
        //                 .await
        //                 .unwrap();
        //         }
        //         _ => {}
        //     }
        // }
    }

    #[tokio::test]
    async fn multiple_subscribe() {
        // Create the MQTT stack
        let state = static_cell::make_static!(State::<
            embassy_sync::blocking_mutex::raw::ThreadModeRawMutex,
            4096,
            4096,
        >::new());
        let (stack, client) = super::new(state);

        // Use the MQTT client to subscribe

        client
            .publish(Publication::new(b"").topic("ABC").finish().unwrap())
            .await
            .unwrap();

        let mut rx_a = [0u8; 512];
        let queue_a = bbqueue::BBQueue::new_from_slice(&mut rx_a);

        let mut rx_b = [0u8; 512];
        let queue_b = bbqueue::BBQueue::new_from_slice(&mut rx_b);

        let fut_a = async {
            let subscribe_a = crate::packets::Subscribe {
                packet_id: 16,
                properties: Properties::Slice(&[]),
                topics: &["ABC".into()],
            };
            // let mut subscription_a = client.subscribe(&queue_a, subscribe_a).await.unwrap();
            // while let Some(message) = subscription_a.next().await {
            //     match message.topic() {
            //         "ABC" => {
            //             client
            //                 .publish(Publication::new(b"").topic("CDE").finish().unwrap())
            //                 .await
            //                 .unwrap();
            //             break;
            //         }
            //         _ => {}
            //     }
            // }
        };

        let fut_b = async {
            let subscribe_b = crate::packets::Subscribe {
                packet_id: 165,
                properties: Properties::Slice(&[]),
                topics: &["CDE".into()],
            };

            // let mut subscription_b = client.subscribe(&queue_b, subscribe_b).await.unwrap();

            // while let Some(message) = subscription_b.next().await {
            //     match message.topic() {
            //         "CDE" => {
            //             client
            //                 .publish(Publication::new(b"").topic("ABC").finish().unwrap())
            //                 .await
            //                 .unwrap();
            //             break;
            //         }
            //         _ => {}
            //     }
            // }
        };

        embassy_futures::join::join(fut_a, fut_b).await;
    }

    // #[tokio::test]
    // async fn mqtt_buffer() {
    //     let mut buf = [0u8; 1024];
    //     let mut buffer = MqttBuffer::new(&mut buf);
    //     let subscribe = crate::packets::Subscribe {
    //         packet_id: 16,
    //         properties: Properties::Slice(&[]),
    //         topics: &["ABC".into()],
    //     };

    //     buffer.write_packet(&subscribe).unwrap();
    //     // buffer.write_packet(&subscribe).unwrap();

    //     let good_subscribe: [u8; 11] = [
    //         0x82, // Subscribe request
    //         0x09, // Remaining length (11)
    //         0x00, 0x10, // Packet identifier (16)
    //         0x00, // Property length
    //         0x00, 0x03, 0x41, 0x42, 0x43, // Topic: ABC
    //         0x00, // Options byte = 0
    //     ];

    //     let packet = buffer.iter().next().unwrap();

    //     assert_eq!(packet.message_type(), MessageType::Subscribe);
    //     // assert_eq!(packet.pid(), Some(16));
    //     // assert_eq!(packet.len(), 11);

    //     let mut assert_buf = [0u8; 128];
    //     let len = packet
    //         .copy_packet(&mut assert_buf, crate::runner::Network)
    //         .await
    //         .unwrap();

    //     assert_eq!(packet.len(), len);
    //     assert_eq!(&assert_buf[..len], good_subscribe);
    // }
}
