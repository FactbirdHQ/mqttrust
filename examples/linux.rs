mod common;

use embedded_nal::{AddrType, Dns, TcpClient};
use mqttrust::{
    EventLoop, MqttOptions, Notification, PublishRequest, QoS, Request, SubscribeRequest,
    SubscribeTopic, TcpSession,
};

use common::network::Network;
use common::timer::SysTimer;
use heapless::{consts, spsc::Queue, String, Vec};
use std::thread;

static mut Q: Queue<Request<Vec<u8, consts::U128>>, consts::U10, u8> =
    Queue(heapless::i::Queue::u8());

fn main() {
    let (mut p, c) = unsafe { Q.split() };

    let network = Network;
    let mut socket = network.socket().unwrap();

    // Connect to broker.hivemq.com:1883
    let broker_addr = network
        .gethostbyname("broker.hivemq.com", AddrType::Either)
        .unwrap();
    network
        .connect(&mut socket, (broker_addr, 1883).into())
        .expect("TCP client cannot connect to the broker");
    let mut session = TcpSession::from(socket);
    let mut mqtt_eventloop =
        EventLoop::new(c, SysTimer::new(), MqttOptions::new("mqtt_test_client_id"));

    nb::block!(mqtt_eventloop.connect(&network, &mut session))
        .expect("MQTT client's connection request failed");

    thread::Builder::new()
        .name("eventloop".to_string())
        .spawn(move || loop {
            match nb::block!(mqtt_eventloop.yield_event(&network, &mut session)) {
                Ok(Notification::Publish(_publish)) => {
                    // defmt::debug!(
                    //     "[{}, {:?}]: {:?}",
                    //     _publish.topic_name,
                    //     _publish.qospid,
                    //     String::from_utf8(_publish.payload).unwrap()
                    // );
                }
                _n => {
                    // defmt::debug!("{:?}", _n);
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
        thread::sleep(std::time::Duration::from_millis(5000));
    }
}
