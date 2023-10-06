#![feature(type_alias_impl_trait)]

mod common;

use embassy_futures::{join, select};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use embedded_mqtt::exp::State;
use embedded_mqtt::publication::Publication;
use embedded_mqtt::types::Properties;
use embedded_mqtt::{packets, QoS};
use futures::StreamExt;
use static_cell::make_static;

const MSG_CNT: u32 = 50;

#[tokio::test]
async fn main() {
    env_logger::init();

    // let mut network = Network::new();

    // let client_id = "mqtt_test_client_id";

    // Create the MQTT stack
    let state = make_static!(State::<NoopRawMutex, 4096, 4096>::new());
    let (stack, client) = embedded_mqtt::exp::new(state);

    let client = make_static!(client);

    let subscribe = crate::packets::Subscribe {
        packet_id: 16,
        properties: Properties::Slice(&[]),
        topics: &[
            "mqttrust/tester/subscriber".into(),
            "mqttrust/tester/subscriber2".into(),
        ],
    };

    let mut subscription = client
        .subscribe(subscribe)
        .await
        .expect("Failed to subscribe to topics!");

    let publish_fut = async {
        for i in 0..MSG_CNT {
            log::debug!("Sending {}", i);
            client
                .publish(
                    Publication::new(format!("{{\"count\": {} }}", i).as_bytes())
                        .topic("mqttrust/tester/subscriber")
                        .qos(QoS::AtLeastOnce)
                        .finish()
                        .unwrap(),
                )
                .await
                .expect("Failed to publish");
            Timer::after(Duration::from_millis(500)).await;
        }
    };

    let subscription_fut = async {
        let mut receive_cnt = 0;
        while let Some(message) = subscription.next().await {
            if message.topic() == "mqttrust/tester/subscriber" {
                receive_cnt += 1;
            }
            if receive_cnt == MSG_CNT {
                break;
            }
        }
        receive_cnt
    };

    let receive_cnt =
        match select::select(stack.run(), join::join(subscription_fut, publish_fut)).await {
            select::Either::First(_) => unreachable!(),
            select::Either::Second((receive_cnt, _)) => receive_cnt,
        };

    assert_eq!(receive_cnt, MSG_CNT);

    println!("Success!");
}
