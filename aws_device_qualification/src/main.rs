mod credentials;

use embassy_futures::select;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embedded_mqtt::{
    transport::embedded_tls::{TlsNalTransport, TlsState},
    Config, DomainBroker, Publish, State, Subscribe, SubscribeTopic,
};
use embedded_tls::{Certificate, TlsConfig};
use futures_util::StreamExt;
use rand::rngs::OsRng;
use static_cell::StaticCell;

const ROUND_TRIP_COUNT: usize = 15;

#[tokio::main(flavor = "current_thread")]
async fn mqttv5() {
    env_logger::init();

    static NETWORK: StaticCell<Network> = StaticCell::new();
    let network = NETWORK.init(Network::new());

    let client_id = "embedded-mqtt";
    let hostname = common::credentials::hostname();

    // Create the MQTT stack
    let broker = DomainBroker::<_, 256>::new(&hostname, &*network).unwrap();
    let config =
        Config::new(client_id, broker).keepalive_interval(embassy_time::Duration::from_secs(50));

    static STATE: StaticCell<State<NoopRawMutex, 1024, 1024, 2>> = StaticCell::new();
    let state = STATE.init(State::<NoopRawMutex, 1024, 1024, 2>::new());
    let (mut stack, client) = embedded_mqtt::new(state, config);

    let idle = async {
        log::debug!("Starting publish!");
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        for i in 0..ROUND_TRIP_COUNT {
            client
                .publish(Publish {
                    dup: false,
                    qos: embedded_mqtt::QoS::AtLeastOnce,
                    pid: None,
                    retain: false,
                    topic_name: "plc/input/embedded_mqtt",
                    payload: format!("This is my super secret payload {i}").as_bytes(),
                    properties: embedded_mqtt::Properties::Slice(&[]),
                })
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    };

    let sub = async {
        let subscribe = Subscribe::new(&[SubscribeTopic {
            topic_path: "plc/input/embedded_mqtt",
            maximum_qos: embedded_mqtt::QoS::AtLeastOnce,
            no_local: false,
            retain_as_published: false,
            retain_handling: embedded_mqtt::RetainHandling::SendAtSubscribeTime,
        }]);

        let mut msg_cnt = 0;
        let mut subscription = client.subscribe::<1>(subscribe).await.unwrap();
        while let Some(message) = subscription.next().await {
            log::info!(
                "Received message {:?} - {:?}",
                message.topic_name(),
                core::str::from_utf8(message.payload())
            );
            msg_cnt += 1;
            if msg_cnt >= ROUND_TRIP_COUNT {
                std::process::exit(0);
            }
        }
    };

    let provider = embedded_tls::UnsecureProvider::new::<embedded_tls::Aes128GcmSha256>(OsRng);

    let ca = common::credentials::root_ca();
    let (pkey, cert) = common::credentials::identity();
    let tls_config = TlsConfig::new()
        .with_server_name(&hostname)
        .with_ca(Certificate::X509(&ca))
        .with_cert(Certificate::X509(&cert))
        .with_priv_key(&pkey);

    let tls_state = TlsState::<16640, 16640>::new();
    let mut transport = TlsNalTransport::new(network, &tls_state, &tls_config, provider);
    stack.run(&mut transport).await;

    embassy_time::with_timeout(
        embassy_time::Duration::from_secs(55),
        select::select(stack.run(&mut transport), idle),
    )
    .await
    .unwrap();
}
