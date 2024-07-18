use std::collections::HashMap;

use rumqttd::{Broker, Config, ConnectionSettings, RouterConfig, ServerSettings};

#[allow(dead_code)]
pub enum MqttVersion {
    V4,
    V5,
}

pub fn start_broker(version: MqttVersion) {
    let (v4, v5) = match version {
        MqttVersion::V4 => {
            let mut v4 = HashMap::new();
            v4.insert(
                "v4".to_string(),
                ServerSettings {
                    name: "v4".to_string(),
                    listen: "0.0.0.0:1883".parse().unwrap(),
                    // tls: Some(rumqttd::TlsConfig::Rustls {
                    //     capath: (),
                    //     certpath: (),
                    //     keypath: (),
                    // }),
                    tls: None,
                    next_connection_delay_ms: 1,
                    connections: ConnectionSettings {
                        connection_timeout_ms: 60000,
                        max_payload_size: 20480,
                        max_inflight_count: 50,
                        auth: None,
                        external_auth: None,
                        dynamic_filters: false,
                    },
                },
            );

            (Some(v4), None)
        }
        MqttVersion::V5 => {
            let mut v5 = HashMap::new();
            v5.insert(
                "v5".to_string(),
                ServerSettings {
                    name: "v5".to_string(),
                    listen: "0.0.0.0:1883".parse().unwrap(),
                    // tls: Some(rumqttd::TlsConfig::Rustls {
                    //     capath: (),
                    //     certpath: (),
                    //     keypath: (),
                    // }),
                    tls: None,
                    next_connection_delay_ms: 1,
                    connections: ConnectionSettings {
                        connection_timeout_ms: 60000,
                        max_payload_size: 20480,
                        max_inflight_count: 50,
                        auth: None,
                        external_auth: None,
                        dynamic_filters: false,
                    },
                },
            );

            (None, Some(v5))
        }
    };

    let router = RouterConfig {
        max_connections: 10010,
        max_outgoing_packet_count: 200,
        max_segment_size: 104857600,
        max_segment_count: 10,
        ..Default::default()
    };

    let config = Config {
        id: 0,
        router,
        v4,
        v5,
        ws: None,
        cluster: None,
        console: None,
        bridge: None,
        prometheus: None,
        metrics: None,
    };

    let mut broker = Broker::new(config);
    std::thread::spawn(move || {
        broker.start().unwrap();
    });
}
