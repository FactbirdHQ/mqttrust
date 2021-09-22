mod common;

use mqttrust::{QoS, SubscribeTopic};
use mqttrust_core::bbqueue::BBBuffer;
use mqttrust_core::Mqtt;
use mqttrust_core::{EventLoop, MqttOptions, Notification};

use common::clock::SysClock;
use common::network::Network;
use std::thread;

static mut Q: BBBuffer<{ 1024 * 6 }> = BBBuffer::new();
const MSG_CNT: u32 = 5;

fn main() {
    let (p, c) = unsafe { Q.try_split_framed().unwrap() };

    let mut network = Network;

    let client_id = "mqtt_test_client_id";

    // Connect to broker.hivemq.com:1883
    let mut mqtt_eventloop = EventLoop::new(
        c,
        SysClock::new(),
        MqttOptions::new(client_id, "broker.hivemq.com".into(), 1883),
    );

    let mqtt_client = mqttrust_core::Client::new(p, client_id);

    nb::block!(mqtt_eventloop.connect(&mut network)).expect("Failed to connect to MQTT");

    let handle = thread::Builder::new()
        .name("eventloop".to_string())
        .spawn(move || {
            let mut receive_cnt = 0;
            while receive_cnt < MSG_CNT {
                match mqtt_eventloop.yield_event(&mut network) {
                    Ok(Notification::Publish(publish)) => {
                        println!("Received {:?}", publish);
                        receive_cnt += 1;
                    }
                    Ok(n) => {
                        println!("{:?}", n);
                    }
                    _ => {}
                }
            }
            receive_cnt
        })
        .unwrap();

    mqtt_client
        .subscribe(&[
            SubscribeTopic {
                topic_path: "mqttrust/tester/subscriber",
                qos: QoS::AtLeastOnce,
            },
            SubscribeTopic {
                topic_path: "mqttrust/tester/subscriber2",
                qos: QoS::AtLeastOnce,
            },
        ])
        .expect("Failed to subscribe to topics!");

    let mut send_cnt = 0;

    while send_cnt < MSG_CNT {
        println!("Sending {}", send_cnt);
        mqtt_client
            .publish(
                "mqttrust/tester/subscriber",
                format!("{{\"count\": {} }}", send_cnt).as_bytes(),
                QoS::AtLeastOnce,
            )
            .expect("Failed to publish");

        send_cnt += 1;
        thread::sleep(std::time::Duration::from_millis(5000));
    }

    let receive_cnt = handle.join().expect("Receiving thread failed!");

    assert_eq!(receive_cnt, send_cnt);
}
