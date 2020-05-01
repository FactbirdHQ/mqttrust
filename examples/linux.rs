mod common;

use embedded_nal::Ipv4Addr;

use mqttrust::{
    MqttEvent, MqttOptions, Notification, PublishRequest, QoS, Request, SubscribeRequest,
    SubscribeTopic,
};

use common::network::Network;
use common::timer::SysTimer;
use heapless::{consts, spsc::Queue};
use std::thread;

static mut Q: Queue<Request, consts::U10> = Queue(heapless::i::Queue::new());

fn main() {
    #[cfg(feature = "logging")]
    env_logger::builder()
        .filter_level(log::LevelFilter::Trace)
        .init();

    let (mut p, c) = unsafe { Q.split() };

    let network = Network;

    // Connect to broker.hivemq.com:1883
    let mut mqtt_eventloop = MqttEvent::new(
        c,
        SysTimer::new(),
        MqttOptions::new("mqtt_test_client_id", Ipv4Addr::new(3, 123, 239, 37), 1883),
    );

    nb::block!(mqtt_eventloop.connect(&network)).expect("Failed to connect to MQTT");

    thread::Builder::new()
        .name("eventloop".to_string())
        .spawn(move || loop {
            match nb::block!(mqtt_eventloop.yield_event(&network)) {
                Ok(Notification::Publish(_publish)) => {
                    #[cfg(feature = "logging")]
                    log::debug!(
                        "[{}, {:?}]: {:?}",
                        _publish.topic_name,
                        _publish.qospid,
                        String::from_utf8(_publish.payload).unwrap()
                    );
                }
                _ => {
                    // log::debug!("{:?}", n);
                }
            }
        })
        .unwrap();

    p.enqueue(
        SubscribeRequest {
            topics: vec![
                SubscribeTopic {
                    topic_path: String::from("mqttrust/tester/subscriber"),
                    qos: QoS::AtLeastOnce,
                },
                SubscribeTopic {
                    topic_path: String::from("mqttrust/tester/subscriber2"),
                    qos: QoS::AtLeastOnce,
                },
            ],
        }
        .into(),
    )
    .expect("Failed to publish!");

    loop {
        p.enqueue(
            PublishRequest::new(
                "mqttrust/tester/whatup".to_owned(),
                "{\"key\": \"value\"}".as_bytes().to_owned(),
            )
            .into(),
        )
        .expect("Failed to publish!");
        thread::sleep(std::time::Duration::from_millis(5000));
    }
}
