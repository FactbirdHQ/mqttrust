use core::cell::RefCell;

use embassy_futures::select::{select3, Either3};
use embassy_sync::{blocking_mutex::raw::RawMutex, mutex::Mutex};
use embedded_io_async::{Read, Write};

use crate::{
    config::Config,
    pubsub::{
        framed::{FramePublisher, FrameSubscriber},
        SliceBufferProvider,
    },
    state::Shared,
    Broker, SUBS,
};

struct Network<S: Read + Write> {
    socket: S,

    // MqttStack should hold an RX buffer just big enough to hold a single
    // packet header + `MAX_TOPIC_LEN`, in order for it to handle `ACK`,
    // `PING`, and route incoming `PUBLISH` messages to a subscriber.
    rx: [u8; 128],
    len: usize,
}

impl<S: Read + Write> Network<S> {
    pub fn new(socket: S) -> Self {
        Self {
            socket,
            rx: [0; 128],
            len: 0,
        }
    }

    pub async fn get(&mut self) -> () {
        match self.socket.read(&mut self.rx[self.len..]).await {
            Ok(n) => {

            }
            Err(_) => {}
        }
    } 
}

pub struct MqttStack<'a, M: RawMutex, B: Broker, S: Read + Write> {
    shared: &'a Mutex<M, RefCell<Shared>>,
    tx_subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, 1>,
    rx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, SUBS>,

    config: Config<B>,

    // Network handle
    network: Network<S>,
}

impl<'a, M: RawMutex, B: Broker, S: Read + Write> MqttStack<'a, M, B, S> {
    pub(crate) fn new(
        socket: S,
        config: Config<B>,
        shared: &'a Mutex<M, RefCell<Shared>>,
        tx_subscriber: FrameSubscriber<'a, M, SliceBufferProvider<'a>, 1>,
        rx_publisher: FramePublisher<'a, M, SliceBufferProvider<'a>, SUBS>,
    ) -> Self {
        Self {
            tx_subscriber,
            rx_publisher,
            shared,
            config,
            network: Network::new(socket),
        }
    }

    pub async fn run(&mut self) -> ! {
        loop {
            match select3(self.network.get(), self.tx_subscriber.read_async(), Self::handle_ping()).await {
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

    async fn handle_ping() {}
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use embedded_nal_async::Ipv4Addr;
    use futures::StreamExt;

    use crate::{
        broker::IpBroker,
        config::Config,
        encoding::v4::{Publish, QoS, Subscribe, SubscribeTopic},
        State,
    };

    struct MockSocket;

    impl embedded_io_async::ErrorType for MockSocket {
        type Error = Infallible;
    }

    impl embedded_io_async::Read for MockSocket {
        async fn read(&mut self, _buf: &mut [u8]) -> Result<usize, Self::Error> {
            Ok(0)
        }
    }

    impl embedded_io_async::Write for MockSocket {
        async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
            Ok(buf.len())
        }
    }

    #[tokio::test]
    async fn subscribe_publish() {
        let socket = MockSocket;

        // Create the MQTT stack
        let state = static_cell::make_static!(State::<NoopRawMutex, 4096, 4096>::new());
        let config = Config::new("client_id", IpBroker::new(Ipv4Addr::LOCALHOST, 1883));
        let (mut stack, client) = crate::new(state, config, socket);

        let fut = async {
            // Use the MQTT client to subscribe

            let subscribe = Subscribe::new(&[SubscribeTopic {
                topic_path: "ABC",
                qos: QoS::AtLeastOnce,
            }]);

            let mut subscription = client.subscribe(subscribe).await.unwrap();

            client
                .publish(Publish {
                    dup: false,
                    qos: QoS::AtLeastOnce,
                    pid: None,
                    retain: false,
                    topic_name: "ABC",
                    payload: b"",
                })
                .await
                .unwrap();

            while let Some(message) = subscription.next().await {
                match message.topic() {
                    "ABC" => {
                        client
                            .publish(Publish {
                                dup: false,
                                qos: QoS::AtLeastOnce,
                                pid: None,
                                retain: false,
                                topic_name: "ABC",
                                payload: b"",
                            })
                            .await
                            .unwrap();
                    }
                    _ => {}
                }
            }
        };

        embassy_futures::select::select(stack.run(), fut).await;
    }

    #[tokio::test]
    async fn multiple_subscribe() {
        let socket = MockSocket;

        // Create the MQTT stack
        let state = static_cell::make_static!(State::<NoopRawMutex, 4096, 4096>::new());
        let config = Config::new("client_id", IpBroker::new(Ipv4Addr::LOCALHOST, 1883));
        let (mut stack, client) = crate::new(state, config, socket);

        // Use the MQTT client to subscribe

        client
            .publish(Publish {
                dup: false,
                qos: QoS::AtLeastOnce,
                pid: None,
                retain: false,
                topic_name: "ABC",
                payload: b"",
            })
            .await
            .unwrap();

        let fut_a = async {
            let subscribe_a = Subscribe::new(&[SubscribeTopic {
                topic_path: "ABC",
                qos: QoS::AtLeastOnce,
            }]);

            let mut subscription_a = client.subscribe(subscribe_a).await.unwrap();
            while let Some(message) = subscription_a.next().await {
                match message.topic() {
                    "ABC" => {
                        client
                            .publish(Publish {
                                dup: false,
                                qos: QoS::AtLeastOnce,
                                pid: None,
                                retain: false,
                                topic_name: "CDE",
                                payload: b"",
                            })
                            .await
                            .unwrap();
                        break;
                    }
                    _ => {}
                }
            }
        };

        let fut_b = async {
            let subscribe_b = Subscribe::new(&[SubscribeTopic {
                topic_path: "CDE",
                qos: QoS::AtLeastOnce,
            }]);

            let mut subscription_b = client.subscribe(subscribe_b).await.unwrap();

            while let Some(message) = subscription_b.next().await {
                match message.topic() {
                    "CDE" => {
                        client
                            .publish(Publish {
                                dup: false,
                                qos: QoS::AtLeastOnce,
                                pid: None,
                                retain: false,
                                topic_name: "ABC",
                                payload: b"",
                            })
                            .await
                            .unwrap();
                        break;
                    }
                    _ => {}
                }
            }
        };

        embassy_futures::select::select(stack.run(), embassy_futures::join::join(fut_a, fut_b))
            .await;
    }
}
