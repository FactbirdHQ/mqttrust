#![feature(type_alias_impl_trait)]
#![feature(async_fn_in_trait)]
mod common;

use std::net::TcpStream;

use embassy_futures::{join, select};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_time::{Duration, Timer};
use embedded_mqtt::{State, Config, NamedBroker};
use futures::StreamExt;
use native_tls::{TlsConnector, TlsStream};
use static_cell::make_static;

use crate::common::credentials;
use crate::common::network::Network;

#[tokio::test]
async fn main() {
    env_logger::init();

    let hostname = credentials::HOSTNAME.unwrap();

    let connector = TlsConnector::builder()
        .identity(credentials::identity())
        .add_root_certificate(credentials::root_ca())
        .build()
        .unwrap();

    let network = Network::new_tls(connector, String::from(hostname));

    let thing_name = "embedded-mqtt";

    let broker = NamedBroker::<&Network<TlsStream<TcpStream>>>::new(hostname, &network).unwrap();
    let config = Config::new(thing_name, broker);

    
    // Create the MQTT stack
    let state = make_static!(State::<NoopRawMutex, 4096, 4096>::new());
    let (mut stack, client) = embedded_mqtt::new(state, config);

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
