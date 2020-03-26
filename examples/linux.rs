extern crate env_logger;

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;

use embedded_nal::{Ipv4Addr, Mode, SocketAddr, SocketAddrV4, TcpStack};

use mqttrust::{Connect, MQTTClient, Protocol, QoS};

use common::timer::SysTimer;

pub struct Network;

pub struct TcpSocket {
    pub stream: Option<TcpStream>,
    mode: Mode,
}

impl TcpSocket {
    pub fn new(mode: Mode) -> Self {
        TcpSocket { stream: None, mode }
    }
}

impl TcpStack for Network {
    type Error = ();
    type TcpSocket = TcpSocket;

    fn open(&self, mode: Mode) -> Result<<Self as TcpStack>::TcpSocket, <Self as TcpStack>::Error> {
        Ok(TcpSocket::new(mode))
    }

    fn read(
        &self,
        network: &mut <Self as TcpStack>::TcpSocket,
        buf: &mut [u8],
    ) -> Result<usize, nb::Error<<Self as TcpStack>::Error>> {
        if let Some(ref mut stream) = network.stream {
            match stream.read(buf) {
                Ok(n) => {
                    if n > 0 {
                        log::info!("READ! {}\r", n);
                    }
                    Ok(n)
                }
                Err(e) => {
                    log::info!("Err! {:?}\r", e);
                    Err(nb::Error::Other(()))
                }
            }
        } else {
            Err(nb::Error::Other(()))
        }
    }

    fn write(
        &self,
        network: &mut <Self as TcpStack>::TcpSocket,
        buf: &[u8],
    ) -> Result<usize, nb::Error<<Self as TcpStack>::Error>> {
        if let Some(ref mut stream) = network.stream {
            Ok(stream.write(buf).map_err(|_| nb::Error::Other(()))?)
        } else {
            Err(nb::Error::Other(()))
        }
    }

    fn is_connected(
        &self,
        _network: &<Self as TcpStack>::TcpSocket,
    ) -> Result<bool, <Self as TcpStack>::Error> {
        Ok(true)
    }

    fn connect(
        &self,
        network: <Self as TcpStack>::TcpSocket,
        remote: SocketAddr,
    ) -> Result<<Self as TcpStack>::TcpSocket, <Self as TcpStack>::Error> {
        Ok(match TcpStream::connect(format!("{}", remote)) {
            Ok(stream) => {
                match network.mode {
                    Mode::Blocking => {
                        stream.set_write_timeout(None).unwrap();
                        stream.set_read_timeout(None).unwrap();
                    }
                    Mode::NonBlocking => panic!("Nonblocking socket mode not supported!"),
                    Mode::Timeout(t) => {
                        stream
                            .set_write_timeout(Some(std::time::Duration::from_millis(t as u64)))
                            .unwrap();
                        stream
                            .set_read_timeout(Some(std::time::Duration::from_millis(t as u64)))
                            .unwrap();
                    }
                };
                TcpSocket {
                    stream: Some(stream),
                    mode: network.mode,
                }
            }
            Err(_e) => return Err(()),
        })
    }

    fn close(
        &self,
        _network: <Self as TcpStack>::TcpSocket,
    ) -> Result<(), <Self as TcpStack>::Error> {
        Ok(())
    }
}

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Trace)
        .init();

    let network = Network;
    let mut mqtt = MQTTClient::new(SysTimer::new(), SysTimer::new(), 512, 512);

    let socket = {
        let soc = network
            .open(Mode::Blocking)
            .expect("Failed to open socket!");

        // Use https://test.mosquitto.org/ to test on port 1883 (MQTT, unencrypted)
        network
            .connect(
                soc,
                SocketAddrV4::new(Ipv4Addr::new(195, 34, 89, 241), 7).into(),
            )
            .expect("Failed to connect to socket!")
    };

    mqtt.connect(&network, socket, Connect {
        protocol: Protocol::MQTT311,
        keep_alive: 60,
        client_id: String::from("T"),
        clean_session: true,
        last_will: None,
        username: None,
        password: None,
    })
    .expect("Failed to connect to MQTT");

    log::info!("MQTT Connected!\r");

    loop {
        mqtt.publish(
            &network,
            QoS::AtLeastOnce,
            format!("fbmini-test"),
            "{\"key\": \"Some json payload\"}".as_bytes().to_owned(),
        )
        .expect("Failed to publish MQTT msg");
        std::thread::sleep(std::time::Duration::from_millis(5000));
    }
}
