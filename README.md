### **Embedded-MQTT**

_A lightweight, `#![no_std]` MQTT client for Rust_

**🚀 Features:**

- **Minimal footprint:** Designed for resource-constrained environments.
- **Asynchronous:** Non-blocking operations for efficient resource utilization.
- **Customizable:** Flexible configuration options to tailor to your needs.
- **Robust:** Handles connection errors and message retransmissions.

**🤝 Contributing:**

We welcome contributions from the community! Please follow these guidelines:

1. **Fork the repository.**
2. **Create a new branch.**
3. **Make your changes.**
4. **Submit a pull request.**

**📄 License:**

This project is licensed under the MIT License. See the LICENSE file for more information.

**☑ TODO:**

- [x] Ping handling
- [x] Connect/Disconnect/Reconnect
- [x] Sending ACK
- [x] Network write
- [x] Topic filter
- [x] Publish before connect handling
- [x] MQTTv5 handling
- [x] Reconnect backoff
- [x] `embedded-tls` support
- [x] Plug-able transport layer
- [x] Deferred publish payload
- [x] Allow client to await connected state
- [x] Check subscribe reason code in SubAck packets
- [x] MQTTv5 Properties
- [x] Setup integration tests & CI
- [x] Packet builders
- [x] Retry on publish & subscribe
- [x] Automatically drop packet if all subscribers touched it in `PubSub`
- [ ] Support "Lass Will"
- [ ] Username/password broker auth support
- [ ] Error handling
- [ ] Logging
- [ ] Docs
- [ ] TEST & Evaluate!
