mod common;

use mqttrust::encoding::v4::{Connack, ConnectReturnCode};
use mqttrust::{Mqtt, QoS, SubscribeTopic};
use mqttrust_core::bbqueue::BBBuffer;
use mqttrust_core::{EventLoop, MqttOptions, Notification};

use common::clock::SysClock;
use common::network::Network;
use native_tls::TlsConnector;
use static_cell::StaticCell;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread;

use crate::common::credentials;

static mut Q: BBBuffer<{ 1024 * 60 }> = BBBuffer::new();

fn main() {
    env_logger::init();

    let (p, c) = unsafe { Q.try_split_framed().unwrap() };

    static HOSTNAME: StaticCell<String> = StaticCell::new();
    let hostname = HOSTNAME.init(credentials::hostname());

    log::info!(
        "Starting device advisor test on endpoint {}",
        hostname.as_str()
    );

    let connector = TlsConnector::builder()
        .identity(credentials::identity())
        .add_root_certificate(credentials::root_ca())
        .build()
        .unwrap();

    let mut network = Network::new_tls(connector, hostname.clone());

    let thing_name = "mqttrust";

    let mut mqtt_eventloop = EventLoop::new(
        c,
        SysClock::new(),
        MqttOptions::new(thing_name, hostname.as_str().into(), 8883),
    );

    let mqtt_client = mqttrust_core::Client::new(p, thing_name);

    let connected = Arc::new(AtomicBool::new(false));
    let con = connected.clone();

    thread::Builder::new()
        .name("eventloop".to_string())
        .spawn(move || loop {
            match nb::block!(mqtt_eventloop.connect(&mut network)) {
                Err(_) => continue,
                Ok(Some(Notification::ConnAck(Connack {
                    session_present,
                    code: ConnectReturnCode::Accepted,
                }))) => {
                    log::info!(
                        "Successfully connected to broker. session_present: {}",
                        session_present
                    );
                    con.store(true, std::sync::atomic::Ordering::Release);
                }
                Ok(n) => {
                    log::info!("Received {:?} during connect", n);
                }
            }

            match nb::block!(mqtt_eventloop.yield_event(&mut network)) {
                Ok(Notification::Publish(_)) => {}
                Ok(n) => {
                    log::trace!("{:?}", n);
                }
                _ => {}
            }
        })
        .unwrap();

    loop {
        thread::sleep(std::time::Duration::from_millis(5000));
        if connected.load(std::sync::atomic::Ordering::Acquire) {
            mqtt_client
                .subscribe(&[SubscribeTopic {
                    topic_path: format!("plc/output/{}", thing_name).as_str(),
                    qos: QoS::AtLeastOnce,
                }])
                .unwrap();

            mqtt_client
                .publish(
                    format!("plc/input/{}", thing_name).as_str(),
                    format!("Hello from {}", thing_name).as_bytes(),
                    QoS::AtLeastOnce,
                )
                .unwrap();
        }
    }
}
