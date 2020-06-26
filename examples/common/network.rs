use embedded_nal::{AddrType, Dns, Mode, SocketAddr, TcpStack};
use heapless::{consts, String};
use no_std_net::IpAddr;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;

use dns_lookup::{lookup_addr, lookup_host};

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

impl Dns for Network {
    type Error = ();

    fn gethostbyaddr(&self, ip_addr: IpAddr) -> Result<String<consts::U256>, Self::Error> {
        let ip: std::net::IpAddr = format!("{}", ip_addr).parse().unwrap();
        let host = lookup_addr(&ip).unwrap();
        Ok(String::from(host.as_str()))
    }
    fn gethostbyname(&self, hostname: &str, _addr_type: AddrType) -> Result<IpAddr, Self::Error> {
        let mut ips: Vec<std::net::IpAddr> = lookup_host(hostname).unwrap();
        format!("{}", ips.pop().unwrap()).parse().map_err(|_| ())
    }
}

impl TcpStack for Network {
    type Error = ();
    type TcpSocket = TcpSocket;

    fn open(&self, mode: Mode) -> Result<Self::TcpSocket, Self::Error> {
        Ok(TcpSocket::new(mode))
    }

    fn read_with<F>(&self, network: &mut Self::TcpSocket, f: F) -> nb::Result<usize, Self::Error>
    where
        F: FnOnce(&[u8], Option<&[u8]>) -> usize,
    {
        let buf = &mut [0u8; 512];
        let len = self.read(network, buf)?;
        Ok(f(&mut buf[..len], None))
    }

    fn read(
        &self,
        network: &mut Self::TcpSocket,
        buf: &mut [u8],
    ) -> Result<usize, nb::Error<Self::Error>> {
        if let Some(ref mut stream) = network.stream {
            stream.read(buf).map_err(|e| match e.kind() {
                ErrorKind::WouldBlock => nb::Error::WouldBlock,
                _ => nb::Error::Other(()),
            })
        } else {
            Err(nb::Error::Other(()))
        }
    }

    fn write(
        &self,
        network: &mut Self::TcpSocket,
        buf: &[u8],
    ) -> Result<usize, nb::Error<Self::Error>> {
        if let Some(ref mut stream) = network.stream {
            Ok(stream.write(buf).map_err(|_| nb::Error::Other(()))?)
        } else {
            Err(nb::Error::Other(()))
        }
    }

    fn is_connected(&self, _network: &Self::TcpSocket) -> Result<bool, Self::Error> {
        Ok(true)
    }

    fn connect(
        &self,
        network: Self::TcpSocket,
        remote: SocketAddr,
    ) -> Result<Self::TcpSocket, Self::Error> {
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

    fn close(&self, _network: Self::TcpSocket) -> Result<(), Self::Error> {
        Ok(())
    }
}
