#![cfg(feature = "mqttv3")]

mod common;

use common::network::Network;
use embassy_futures::select;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embedded_mqtt::{
    transport::embedded_nal::NalTransport, Config, IpBroker, Publish, State, Subscribe,
    SubscribeTopic,
};
use embedded_nal_async::Ipv4Addr;
use futures::StreamExt;
use static_cell::StaticCell;

const ROUND_TRIP_COUNT: usize = 15;

#[tokio::test(flavor = "current_thread")]
async fn main() {
    env_logger::init();

    // TODO: Configure a RUMQTTD broker instead of having to manually start a mosquitto broker. This also allows testing with TLS.

    static NETWORK: StaticCell<Network> = StaticCell::new();
    let network = NETWORK.init(Network::new());

    let client_id = "mqtt_test_client_id_v3";

    // Create the MQTT stack
    let broker = IpBroker::new(Ipv4Addr::new(127, 0, 0, 1), 1883);
    let config =
        Config::new(client_id, broker).keepalive_interval(embassy_time::Duration::from_secs(50));

    static STATE: StaticCell<State<NoopRawMutex, 1024, 1024, 2>> = StaticCell::new();
    let state = STATE.init(State::<NoopRawMutex, 1024, 1024, 2>::new());
    let (mut stack, client) = embedded_mqtt::new(state, config);

    let idle = async {
        log::debug!("Starting publish!");
        for i in 0..ROUND_TRIP_COUNT {
            client
                .publish(Publish {
                    dup: false,
                    qos: embedded_mqtt::QoS::AtLeastOnce,
                    pid: None,
                    retain: false,
                    topic_name: "embedded_mqtt/embassy_async/hello",
                    payload: format!("This is my super secret payload {i}").as_bytes(),
                })
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };

    let sub = async {
        let subscribe = Subscribe::new(&[SubscribeTopic {
            topic_path: "embedded_mqtt/embassy_async/hello",
            maximum_qos: embedded_mqtt::QoS::AtLeastOnce,
        }]);

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

    let mut transport = NalTransport::new(network);

    embassy_time::with_timeout(
        embassy_time::Duration::from_secs(55),
        select::select3(stack.run(&mut transport), idle, sub),
    )
    .await
    .unwrap();
}
