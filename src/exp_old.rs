
mod exp {
    use core::{cell::RefCell, future::poll_fn, task::Poll};

    use embassy_sync::waitqueue::WakerRegistration;

    use crate::{
        de::{packet_header::PacketHeader, received_packet::ReceivedPacket},
        message_types::ControlPacket,
        packets::{self, Subscribe},
        publication::Publication,
        ring_buffer::RingBuffer,
        runner::Network,
        types::Properties,
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Error {
        Todo,
    }

    pub struct Subscriptions<'a> {
        rx_buffer: MqttBuffer<'a>,
        rx_waker: WakerRegistration,
        // buffer: &'a mut [u8],
    }

    impl<'a> Subscriptions<'a> {
        pub fn add(&mut self, topic_filter: &str) -> Result<(), Error> {
            Ok(())
        }
    }

    struct SubscriptionHandle(pub u8);

    struct SubStorage<'a> {
        inner: Option<Subscriptions<'a>>,
    }

    impl<'a> SubStorage<'a> {
        pub const EMPTY: Self = Self { inner: None };
    }

    pub struct SubResources<const MAX_SUBSCRIPTIONS: usize> {
        subs: [SubStorage<'static>; MAX_SUBSCRIPTIONS],
    }

    impl<const MAX_SUBSCRIPTIONS: usize> SubResources<MAX_SUBSCRIPTIONS> {
        pub const fn new() -> Self {
            Self {
                subs: [SubStorage::EMPTY; MAX_SUBSCRIPTIONS],
            }
        }
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
            let (offset, message) =
                crate::ser::MqttSerializer::to_buffer_meta(buf, packet).unwrap();
            let len = offset + message.len();
            self.0.push(len);
            Ok(len)
        }

        pub(crate) fn iter(&self) -> PacketIter {
            PacketIter {
                buf: self.0.buf(),
                idx: 3,
            }
        }
    }

    pub(crate) struct PacketIter<'a> {
        buf: &'a [u8],
        idx: usize,
    }

    impl<'a> Iterator for PacketIter<'a> {
        type Item = PacketHeader<'a>;

        fn next(&mut self) -> Option<Self::Item> {
            let header = PacketHeader::from_buffer(&self.buf[self.idx..]).ok()?;
            self.idx += header.len();
            Some(header)
        }
    }

    struct ClientState<'a> {
        subscriptions: &'a mut [SubStorage<'a>],
        tx_buffer: MqttBuffer<'a>,

        tx_waker: WakerRegistration,
    }

    impl<'a> ClientState<'a> {
        pub fn new(
            tx_buffer: impl Into<MqttBuffer<'a>>,
            subs_storage: &'a mut [SubStorage<'a>],
        ) -> ClientState<'a> {
            Self {
                subscriptions: subs_storage,

                tx_buffer: tx_buffer.into(),
                tx_waker: WakerRegistration::new(),
            }
        }

        pub fn new_tx(tx_buffer: impl Into<MqttBuffer<'a>>) -> ClientState<'a> {
            Self {
                subscriptions: &mut [],

                tx_buffer: tx_buffer.into(),
                tx_waker: WakerRegistration::new(),
            }
        }

        pub fn add_subscription(&mut self, subscription: Subscriptions<'a>) -> SubscriptionHandle {
            for (index, slot) in self.subscriptions.iter_mut().enumerate() {
                if slot.inner.is_none() {
                    let handle = SubscriptionHandle(index as u8);
                    *slot = SubStorage {
                        inner: Some(subscription),
                    };

                    return handle;
                }
            }
            panic!("adding a subscription to a full Client");
        }

        /// Register a waker for receive operations.
        ///
        /// The waker is woken on state changes that might affect the return value
        /// of `recv` method calls, such as receiving data, or the socket closing.
        ///
        /// Notes:
        ///
        /// - Only one waker can be registered at a time. If another waker was previously registered,
        ///   it is overwritten and will no longer be woken.
        /// - The Waker is woken only once. Once woken, you must register it again to receive more wakes.
        /// - "Spurious wakes" are allowed: a wake doesn't guarantee the result of `recv` has
        ///   necessarily changed.
        // pub fn register_recv_waker(&mut self, waker: &core::task::Waker) {
        //     if let Some(ref mut s) = self.subscriptions {
        //         s.rx_waker.register(waker)
        //     }
        // }

        /// Register a waker for send operations.
        ///
        /// The waker is woken on state changes that might affect the return value
        /// of `send` method calls, such as space becoming available in the transmit
        /// buffer, or the socket closing.
        ///
        /// Notes:
        ///
        /// - Only one waker can be registered at a time. If another waker was previously registered,
        ///   it is overwritten and will no longer be woken.
        /// - The Waker is woken only once. Once woken, you must register it again to receive more wakes.
        /// - "Spurious wakes" are allowed: a wake doesn't guarantee the result of `send` has
        ///   necessarily changed.
        pub fn register_send_waker(&mut self, waker: &core::task::Waker) {
            self.tx_waker.register(waker)
        }
    }

    struct ClientStorage<'a> {
        inner: Option<ClientState<'a>>,
    }

    impl<'a> ClientStorage<'a> {
        pub const EMPTY: Self = Self { inner: None };
    }

    struct ClientSet<'a> {
        pub clients: &'a mut [ClientStorage<'a>],
    }

    #[derive(Copy, Clone)]
    struct ClientHandle(u8);

    impl<'a> ClientSet<'a> {
        pub fn new(clients: &'a mut [ClientStorage<'a>]) -> ClientSet<'a> {
            Self { clients }
        }

        pub fn add(&mut self, client_state: ClientState<'a>) -> ClientHandle {
            for (index, slot) in self.clients.iter_mut().enumerate() {
                if slot.inner.is_none() {
                    let handle = ClientHandle(index as u8);
                    *slot = ClientStorage {
                        inner: Some(client_state),
                    };

                    return handle;
                }
            }
            panic!("adding a client to a full ClientSet");
        }

        pub fn get(&self, handle: ClientHandle) -> &ClientState<'a> {
            self.clients[handle.0 as usize].inner.as_ref().unwrap()
        }

        pub fn get_mut(&mut self, handle: ClientHandle) -> &mut ClientState<'a> {
            self.clients[handle.0 as usize].inner.as_mut().unwrap()
        }

        // pub fn remove(&mut self, handle: ClientHandle) -> ClientState<'_> {
        //     self.clients[handle.0 as usize].inner.take().unwrap()
        // }
    }

    pub struct ClientResources<const MAX_CLIENTS: usize> {
        clients: [ClientStorage<'static>; MAX_CLIENTS],
    }

    impl<const MAX_CLIENTS: usize> ClientResources<MAX_CLIENTS> {
        pub const fn new() -> Self {
            Self {
                clients: [ClientStorage::EMPTY; MAX_CLIENTS],
            }
        }
    }

    struct Clients {
        clients: ClientSet<'static>,
        waker: WakerRegistration,
    }

    pub struct MqttStack {
        client_refs: RefCell<Clients>,
    }

    impl MqttStack {
        pub fn new<const MAX_CLIENTS: usize>(
            resources: &'static mut ClientResources<MAX_CLIENTS>,
        ) -> Self {
            let clients = ClientSet::new(&mut resources.clients[..]);

            let client_refs = Clients {
                clients,
                waker: WakerRegistration::new(),
            };

            Self {
                client_refs: RefCell::new(client_refs),
            }
        }

        pub async fn run(&self) -> ! {
            loop {}
        }
    }

    #[derive(Clone, Copy)]
    struct ClientIo<'a> {
        resources: &'a RefCell<Clients>,
        handle: ClientHandle,
    }

    impl<'a> ClientIo<'a> {
        fn with<R>(&self, f: impl FnOnce(&ClientState) -> R) -> R {
            let s = &*self.resources.borrow();
            let client_state = s.clients.get(self.handle);
            f(client_state)
        }

        fn with_mut<R>(&self, f: impl FnOnce(&mut ClientState) -> R) -> R {
            let s = &mut *self.resources.borrow_mut();
            let socket = s.clients.get_mut(self.handle);
            let res = f(socket);
            s.waker.wake();
            res
        }
    }

    pub struct MqttClient<'a> {
        client_io: ClientIo<'a>,
    }

    impl<'a> MqttClient<'a> {
        pub fn new<const MAX_SUBS: usize>(
            stack: &'a MqttStack,
            tx_buffer: &'a mut [u8],
            sub_resources: &'static mut SubResources<MAX_SUBS>,
        ) -> Self {
            let s = &mut *stack.client_refs.borrow_mut();
            let tx_buffer: &'static mut [u8] = unsafe { core::mem::transmute(tx_buffer) };
            let state = ClientState::new(MqttBuffer::new(tx_buffer), &mut sub_resources.subs[..]);

            let handle = s.clients.add(state);

            Self {
                client_io: ClientIo {
                    resources: &stack.client_refs,
                    handle,
                },
            }
        }

        pub async fn publish(&self) -> Result<(), Error> {
            Ok(())
        }

        pub async fn subscribe(
            &self,
            rx_buffer: &mut [u8],
            packet: impl Into<Subscribe<'_>>,
        ) -> Result<Subscription<'a>, Error> {
            let sub = packet.into();
            let rx_buffer: &'static mut [u8] = unsafe { core::mem::transmute(rx_buffer) };

            let subsciption = Subscriptions {
                rx_buffer: MqttBuffer::new(rx_buffer),
                rx_waker: WakerRegistration::new(),
                // buffer: (),
            };

            // let handle = poll_fn(move |cx| {
            //     self.client_io.with_mut(|state| {
            //         let handle = state.add_subscription(subsciption);

            //         // TODO: Make this part atomic/clean up
            //         // if let Some(subs) = &mut state.subscriptions {
            //         //     for filter in sub.topics {
            //         //         subs.add(filter.topic())?;
            //         //     }
            //         // }
            //         match state.tx_buffer.write_packet(&sub) {
            //             // Not ready to send (no space in the tx buffer)
            //             Ok(0) => {
            //                 state.register_send_waker(cx.waker());
            //                 Poll::Pending
            //             }
            //             // Some data sent
            //             Ok(n) => Poll::Ready(Ok(handle)),
            //             Err(_) => Poll::Ready(Err(Error::Todo)),
            //         }
            //     })
            // })
            // .await?;

            Ok(Subscription {
                client_io: self.client_io,
                handle: SubscriptionHandle(0),
            })
        }
    }

    impl<'a> Drop for MqttClient<'a> {
        fn drop(&mut self) {
            let mut stack = self.client_io.resources.borrow_mut();
            // stack.clients.remove(self.client_io.handle);
            stack.waker.wake();
        }
    }

    pub struct Subscription<'a> {
        client_io: ClientIo<'a>,
        handle: SubscriptionHandle,
    }

    impl<'a> futures::Stream for Subscription<'a> {
        type Item = packets::Pub<'a, &'a [u8]>;

        fn poll_next(
            self: core::pin::Pin<&mut Self>,
            cx: &mut core::task::Context<'_>,
        ) -> Poll<Option<Self::Item>> {
            todo!()
        }
    }

    impl<'a> Drop for Subscription<'a> {
        fn drop(&mut self) {
            // Unsubscribe
            self.client_io.with_mut(|state| {})
        }
    }

    #[cfg(test)]
    mod tests {
        use futures::{StreamExt, TryStreamExt};

        use crate::message_types::MessageType;

        use super::*;

        #[tokio::test]
        async fn subscribe_publish() {
            // Create the MQTT stack
            static mut RESOURCES: ClientResources<3> = ClientResources::new();
            let stack = MqttStack::new(unsafe { &mut RESOURCES });

            // Instantiate one or more MQTT clients to the MQTT stack
            let mut tx_a = [0u8; 512];
            static mut SUB_RESOURCES: SubResources<3> = SubResources::new();
            let client_a = MqttClient::new(&stack, &mut tx_a, unsafe { &mut SUB_RESOURCES });

            // Use the MQTT client to subscribe
            let subscribe = crate::packets::Subscribe {
                packet_id: 16,
                properties: Properties::Slice(&[]),
                topics: &["ABC".into()],
            };

            let mut rx_a = [0u8; 512];
            let mut subscription = client_a.subscribe(&mut rx_a, subscribe).await.unwrap();

            client_a.publish().await.unwrap();

            while let Some(packet) = subscription.next().await {
                match packet.topic.0 {
                    "ABC" => {
                        client_a.publish().await.unwrap();
                    }
                    _ => {}
                }
            }

            drop(subscription);
        }

        #[tokio::test]
        async fn mqtt_buffer() {
            let mut buf = [0u8; 1024];
            let mut buffer = MqttBuffer::new(&mut buf);
            let subscribe = crate::packets::Subscribe {
                packet_id: 16,
                properties: Properties::Slice(&[]),
                topics: &["ABC".into()],
            };

            buffer.write_packet(&subscribe).unwrap();
            // buffer.write_packet(&subscribe).unwrap();

            let good_subscribe: [u8; 11] = [
                0x82, // Subscribe request
                0x09, // Remaining length (11)
                0x00, 0x10, // Packet identifier (16)
                0x00, // Property length
                0x00, 0x03, 0x41, 0x42, 0x43, // Topic: ABC
                0x00, // Options byte = 0
            ];

            let packet = buffer.iter().next().unwrap();

            assert_eq!(packet.message_type(), MessageType::Subscribe);
            // assert_eq!(packet.pid(), Some(16));
            // assert_eq!(packet.len(), 11);

            let mut assert_buf = [0u8; 128];
            let len = packet
                .copy_packet(&mut assert_buf, crate::runner::Network)
                .await
                .unwrap();

            assert_eq!(packet.len(), len);
            assert_eq!(&assert_buf[..len], good_subscribe);
        }
    }
}
