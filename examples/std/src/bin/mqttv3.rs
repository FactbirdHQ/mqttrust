#![allow(async_fn_in_trait)]
#![feature(type_alias_impl_trait)]

#[path = "../network.rs"]
mod network;

use embassy_futures::select;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embedded_mqtt::{Config, IpBroker, Publish, State, Subscribe, SubscribeTopic};
use embedded_nal_async::Ipv4Addr;
use futures::StreamExt;
use network::Network;
use static_cell::make_static;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::init();

    let network = make_static!(Network::new(), #[export_name = "network"]);

    let client_id = "mqtt_test_client_id";

    // Create the MQTT stack
    let broker = IpBroker::new(Ipv4Addr::new(127, 0, 0, 1), 1883);
    let config =
        Config::new(client_id, broker).keepalive_interval(embassy_time::Duration::from_secs(50));

    let state = make_static!(State::<NoopRawMutex, 1024, 1024, 2>::new(), #[export_name = "mqtt_state"]);
    let (mut stack, client) = embedded_mqtt::new(state, config, &*network);

    // let client = make_static!(client);

    let idle = async {
        log::debug!("Starting publish!");
        for i in 0.. {
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
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    };

    let sub = async {
        let subscribe = Subscribe::new(&[SubscribeTopic {
            topic_path: "embedded_mqtt/embassy_async/hello",
            qos: embedded_mqtt::QoS::AtLeastOnce,
        }]);

        let mut subscription = client.subscribe(subscribe).await.unwrap();
        while let Some(message) = subscription.next().await {
            log::info!("Received message {:?} - {:?}", message.topic(), core::str::from_utf8(message.payload()));
        }
    };

    select::select3(stack.run(), idle, sub).await;
}
