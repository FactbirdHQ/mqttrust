mod common;

use mqttrust::{
    MqttEvent, MqttOptions, Notification, PublishRequest, QoS, Request, SubscribeRequest,
    SubscribeTopic,
};

use common::network::Network;
use common::timer::SysTimer;
use heapless::{consts, spsc::Queue, String, Vec};
use std::thread;

static mut Q: Queue<Request<Vec<u8, consts::U128>>, consts::U10> = Queue(heapless::i::Queue::new());

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
        MqttOptions::new("mqtt_test_client_id", "broker.hivemq.com".into(), 1883),
    );

    #[cfg(feature = "logging")]
    log::info!("eventloop created");

    nb::block!(mqtt_eventloop.connect(&network)).expect("Failed to connect to MQTT");

    #[cfg(feature = "logging")]
    log::info!("Connected");

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
                _n => {
                    #[cfg(feature = "logging")]
                    log::debug!("{:?}", _n);
                }
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
        .into(),
    )
    .expect("Failed to publish!");

    loop {
        p.enqueue(
            PublishRequest::new(
                String::from("mqttrust/tester/whatup"),
                Vec::from_slice(b"{\"key\": \"value\"}").unwrap(),
            )
            .into(),
        )
        .expect("Failed to publish!");
        #[cfg(feature = "logging")]
        log::info!("Publishing");
        thread::sleep(std::time::Duration::from_millis(5000));
    }
}
