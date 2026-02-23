#![cfg(feature = "mqttv3")]

mod broker;
mod common;

use common::network::Network;
use core::net::Ipv4Addr;
use embassy_futures::select;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use futures_util::StreamExt;
use mqttrust::{
    transport::embedded_nal::NalTransport, Config, IpBroker, Publish, State, Subscribe,
    SubscribeTopic,
};
use static_cell::StaticCell;

const ROUND_TRIP_COUNT: usize = 15;

#[tokio::test(flavor = "current_thread")]
async fn main() {
    env_logger::init();

    broker::start_broker(broker::MqttVersion::V4);

    static NETWORK: StaticCell<Network> = StaticCell::new();
    let network = NETWORK.init(Network::new());

    let client_id = "mqtt_test_client_id_v3";

    // Create the MQTT stack
    let broker = IpBroker::new(Ipv4Addr::new(0, 0, 0, 0), 1883);
    let config = Config::builder()
        .client_id(client_id.try_into().unwrap())
        .keepalive_interval(embassy_time::Duration::from_secs(50))
        .build();

    static STATE: StaticCell<State<NoopRawMutex, 1024, 1024>> = StaticCell::new();
    let state = STATE.init(State::new());
    let (mut stack, client) = mqttrust::new(state, config);

    let idle = async {
        log::debug!("Starting publish!");
        for i in 0..ROUND_TRIP_COUNT {
            client
                .publish(
                    Publish::builder()
                        .topic_name("mqttrust/embassy_async/hello")
                        .payload(format!("This is my super secret payload {i}").as_bytes())
                        .build(),
                )
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };

    let sub = async {
        let topics = [SubscribeTopic::builder()
            .topic_path("mqttrust/embassy_async/hello")
            .build()];
        let subscribe = Subscribe::builder().topics(&topics).build();

        let mut msg_cnt = 0;
        let mut subscription = client.subscribe::<1>(subscribe).await.unwrap();
        while let Some(message) = subscription.next().await {
            log::info!(
                "Received message {:?} - {:?}",
                message.topic_name(),
                core::str::from_utf8(message.payload())
            );
            msg_cnt += 1;
            if msg_cnt >= ROUND_TRIP_COUNT * 2 {
                std::process::exit(0);
            }
        }
    };

    let mut transport = NalTransport::new(network, broker);

    embassy_time::with_timeout(
        embassy_time::Duration::from_secs(55),
        select::select3(stack.run(&mut transport), idle, sub),
    )
    .await
    .unwrap();

    stack.disconnect(&mut transport).await.unwrap();
}
