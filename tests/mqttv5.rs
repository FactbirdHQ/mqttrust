#![cfg(feature = "mqttv5")]

mod broker;
mod common;

use common::network::Network;
use embassy_futures::select;
use embassy_sync::{
    blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex},
    signal::Signal,
};
use embedded_mqtt::{
    transport::embedded_nal::NalTransport, Config, IpBroker, Publish, State, Subscribe,
    SubscribeTopic,
};
use embedded_nal_async::Ipv4Addr;
use futures_util::StreamExt;
use static_cell::StaticCell;

const ROUND_TRIP_COUNT: usize = 15;

#[tokio::test(flavor = "current_thread")]
async fn mqttv5() {
    env_logger::init();

    broker::start_broker(broker::MqttVersion::V5);

    static NETWORK: StaticCell<Network> = StaticCell::new();
    let network = NETWORK.init(Network::new());

    let client_id = "mqtt_test_client_id_v5";

    // Create the MQTT stack
    let broker = IpBroker::new(Ipv4Addr::new(0, 0, 0, 0), 1883);
    let config = Config::builder()
        .client_id(client_id.try_into().unwrap())
        .keepalive_interval(embassy_time::Duration::from_secs(50))
        .build();
    static STATE: StaticCell<State<NoopRawMutex, 1024, 1024, 8>> = StaticCell::new();
    let state = STATE.init(State::new());
    let (mut stack, client) = embedded_mqtt::new(state, config);

    let connected_signal = Signal::<CriticalSectionRawMutex, ()>::new();

    let idle = async {
        log::debug!("Starting publish!");
        connected_signal.wait().await;

        for i in 0..ROUND_TRIP_COUNT {
            log::debug!("################# PUBLISH ROUND {}!", i);

            client
                .publish(
                    Publish::builder()
                        .topic_name("embedded_mqtt/embassy_async/hello")
                        .payload(format!("This is my super secret hello payload {i}").as_bytes())
                        .build(),
                )
                .await
                .unwrap();

            client
                .publish(
                    Publish::builder()
                        .topic_name("embedded_mqtt/embassy_async/other_topic")
                        .payload(
                            format!("This is my super secret other_topic payload {i}").as_bytes(),
                        )
                        .build(),
                )
                .await
                .unwrap();

            // client
            //     .publish(
            //         Publish::builder()
            //             .topic_name("embedded_mqtt/embassy_async_no_subs/hello")
            //             .payload(format!("This is my super secret payload {i}").as_bytes())
            //             .build(),
            //     )
            //     .await
            //     .unwrap();

            log::debug!("#################");

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }

        loop {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    };

    let sub = async {
        let topic_paths = [SubscribeTopic::builder()
            .topic_path("embedded_mqtt/embassy_async/#")
            .build()];

        let subscribe = Subscribe::builder().topics(&topic_paths).build();

        client.wait_connected().await;

        let mut msg_cnt = 0;
        let mut subscription = client.subscribe::<1>(subscribe).await.unwrap();
        connected_signal.signal(());
        while let Some(message) = subscription.next().await {
            log::info!(
                "Received message {:?} - {:?}",
                message.topic_name(),
                core::str::from_utf8(message.payload())
            );
            msg_cnt += 1;
            if msg_cnt >= ROUND_TRIP_COUNT * 2 {
                return Ok(());
            }
        }

        Err(())
    };

    let mut transport = NalTransport::new(network, broker);

    match embassy_time::with_timeout(
        embassy_time::Duration::from_secs(55),
        select::select3(stack.run(&mut transport), idle, sub),
    )
    .await
    .unwrap()
    {
        select::Either3::First(_) => unreachable!(),
        select::Either3::Second(_) => unreachable!(),
        select::Either3::Third(r) => r.unwrap(),
    }

    stack.disconnect(&mut transport).await.unwrap();
}
