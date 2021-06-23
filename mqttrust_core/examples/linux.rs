mod common;

use mqttrust::{PublishRequest, QoS, SubscribeRequest, SubscribeTopic};
use mqttrust_core::OwnedRequest;
use mqttrust_core::{EventLoop, MqttOptions, Notification};

use common::clock::SysClock;
use common::network::Network;
use heapless::{spsc::Queue, String, Vec};
use std::convert::TryInto;
use std::thread;

static mut Q: Queue<OwnedRequest<128, 512>, 10> = Queue::new();

fn main() {
    let (mut p, c) = unsafe { Q.split() };

    let mut network = Network;

    // Connect to broker.hivemq.com:1883
    let mut mqtt_eventloop = EventLoop::new(
        c,
        SysClock::new(),
        MqttOptions::new("mqtt_test_client_id", "broker.hivemq.com".into(), 1883),
    );

    nb::block!(mqtt_eventloop.connect(&mut network)).expect("Failed to connect to MQTT");

    thread::Builder::new()
        .name("eventloop".to_string())
        .spawn(move || loop {
            match mqtt_eventloop.yield_event(&mut network) {
                Ok(Notification::Publish(publish)) => {
                    println!("Received {:?}", publish);
                }
                Ok(n) => {
                    println!("{:?}", n);
                }
                _ => {}
            }
        })
        .unwrap();

    p.enqueue(
        SubscribeRequest {
            topics: Vec::from_slice(&[
                SubscribeTopic {
                    topic_path: String::from("mqttrust/tester/subscriber"),
                    qos: QoS::AtLeastOnce,
                },
                SubscribeTopic {
                    topic_path: String::from("mqttrust/tester/subscriber2"),
                    qos: QoS::AtLeastOnce,
                },
            ])
            .unwrap(),
        }
        .try_into()
        .unwrap(),
    )
    .expect("Failed to publish!");

    let mut cnt = 0;

    loop {
        println!("Enqueing {}", cnt);
        p.enqueue(
            PublishRequest::new(
                "mqttrust/tester/whatup",
                format!("{{\"count\": {} }}", cnt).as_bytes(),
            )
            .try_into()
            .unwrap(),
        )
        .expect("Failed to publish!");
        cnt += 1;
        thread::sleep(std::time::Duration::from_millis(5000));
    }
}
