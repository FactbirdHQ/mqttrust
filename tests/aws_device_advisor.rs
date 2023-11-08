#![feature(type_alias_impl_trait)]
#![feature(impl_trait_projections)]
#![feature(async_fn_in_trait)]
mod common;

use embassy_futures::{join, select};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use embedded_mqtt::{Config, DomainBroker, State};
use static_cell::make_static;

use crate::common::{
    credentials,
    network::{StdDns, StdTcpConnect},
};

#[tokio::test]
async fn main() {
    env_logger::init();

    let hostname = credentials::HOSTNAME.unwrap();

    let network = StdTcpConnect::new();

    let thing_name = "embedded-mqtt";

    let broker = DomainBroker::<&StdDns<()>>::new(hostname, &StdDns::new(())).unwrap();
    let config = Config::new(thing_name, broker);

    // Create the MQTT stack
    let state = make_static!(State::<NoopRawMutex, 4096, 4096, 4>::new());
    let (mut stack, client) = embedded_mqtt::new(state, config, &network);

    let subscribe_topic = format!("{}/device/advisor", thing_name);
    let publish_topic = format!("{}/device/advisor/hello", thing_name);

    let subscribe = crate::packets::Subscribe {
        packet_id: 16,
        properties: Properties::Slice(&[]),
        topics: &[subscribe_topic.as_str().into()],
    };

    let mut subscription = client
        .subscribe(subscribe)
        .await
        .expect("Failed to subscribe to topics!");

    let publish_fut = async {
        client
            .publish(
                Publication::new(format!("Hello from {}", thing_name).as_bytes())
                    .topic(publish_topic.as_str())
                    .qos(QoS::AtLeastOnce)
                    .finish()
                    .unwrap(),
            )
            .await
            .expect("Failed to publish");

        Timer::after(Duration::from_secs(5)).await;
    };

    let subscription_fut = async {
        while let Some(message) = subscription.next().await {
            if message.topic() == subscribe_topic {
                break;
            }
        }
    };

    select::select(stack.run(), join::join(subscription_fut, publish_fut)).await;
}
