# mqttrust

A lightweight, `#![no_std]` async MQTT client for embedded systems.

## Features

- Minimal footprint designed for resource-constrained environments
- Fully asynchronous, built on top of the Embassy async ecosystem
- Supports both MQTTv3.1.1 and MQTTv5
- Pluggable transport layer (bring your own TCP/TLS stack)
- Optional `embedded-tls` integration for secure connections
- QoS 0, 1, and 2 (QoS 2 via feature flag)
- Configurable buffer sizes via compile-time feature flags
- Automatic reconnection with configurable backoff
- Topic filter matching with wildcard support (`+` and `#`)
- `defmt` and `log` logging support

## Quick start

```rust,ignore
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use mqttrust::{
    transport::embedded_nal::NalTransport, Config, IpBroker, Publish, State, Subscribe,
};

// Create state with TX/RX buffer sizes
static STATE: StaticCell<State<NoopRawMutex, 1024, 1024>> = StaticCell::new();
let state = STATE.init(State::new());

// Configure the client
let broker = IpBroker::new(Ipv4Addr::new(192, 168, 1, 100), 1883);
let config = Config::builder()
    .client_id("my-device".try_into().unwrap())
    .keepalive_interval(Duration::from_secs(30))
    .build();

let (mut mqtt_stack, client) = mqttrust::new(state, config);

// Run the stack in a background task
spawner.spawn(mqtt_task(mqtt_stack, broker)).unwrap();

// Subscribe and publish
let topics = ["sensor/temperature".into()];
let mut subscription = client
    .subscribe::<1>(Subscribe::builder().topics(&topics).build())
    .await
    .unwrap();

client
    .publish(Publish::builder().topic_name("sensor/temperature").payload(b"23.5").build())
    .await
    .unwrap();
```

## Feature flags

| Feature | Default | Description |
|---------|---------|-------------|
| `mqttv5` | Yes | Enable MQTTv5 protocol support |
| `mqttv3` | No | Enable MQTTv3.1.1 protocol support |
| `embedded-tls` | Yes | TLS transport via `embedded-tls` |
| `qos2` | No | Enable QoS 2 (Exactly Once) support |
| `std` | No | Enable standard library support |
| `defmt` | No | Logging via `defmt` |
| `log` | No | Logging via the `log` crate |
| `thumbv6` | No | Cortex-M0/M0+ support |

Exactly one of `mqttv5` or `mqttv3` must be enabled.

## Buffer size configuration

Internal buffer sizes are configured via feature flags. Each setting has a default
value (shown below) and can be overridden by enabling the corresponding feature:

| Prefix | Default | Options |
|--------|---------|---------|
| `max-client-id-len-` | 64 | 16, 32, 64, 128, 256 |
| `max-topic-len-` | 128 | 32, 64, 128, 256 |
| `max-sub-topics-per-msg-` | 8 | 2, 4, 8, 16 |
| `max-subscribers-` | 8 | 2, 4, 8, 16 |
| `max-inflight-` | 8 | 2, 4, 8, 16 |
| `max-pubsub-messages-` | 16 | 8, 16, 32, 64, 128, 256 |

For example, to allow up to 256-byte topic names:

```toml
[dependencies]
mqttrust = { version = "1.0", features = ["mqttv5", "max-topic-len-256"] }
```

## Contributing

Contributions are welcome. Please follow these guidelines:

1. Fork the repository.
2. Create a new branch.
3. Make your changes.
4. Submit a pull request.

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.
