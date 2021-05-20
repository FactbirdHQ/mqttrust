use embedded_nal::{AddrType, Dns, IpAddr, SocketAddr, TcpClientStack};
use heapless::String;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;

use dns_lookup::{lookup_addr, lookup_host};

pub struct Network;

pub struct TcpSocket {
    pub stream: Option<TcpStream>,
}

impl TcpSocket {
    pub fn new() -> Self {
        TcpSocket { stream: None }
    }
}

impl Dns for Network {
    type Error = ();

    fn gethostbyaddr(&self, ip_addr: IpAddr) -> Result<String<256>, Self::Error> {
        let ip: std::net::IpAddr = format!("{}", ip_addr).parse().unwrap();
        let host = lookup_addr(&ip).unwrap();
        Ok(String::from(host.as_str()))
    }
    fn gethostbyname(&self, hostname: &str, _addr_type: AddrType) -> Result<IpAddr, Self::Error> {
        let mut ips: Vec<std::net::IpAddr> = lookup_host(hostname).unwrap();
        format!("{}", ips.pop().unwrap()).parse().map_err(|_| ())
    }
}

impl TcpClientStack for Network {
    type Error = ();
    type TcpSocket = TcpSocket;

    fn socket(&self) -> Result<Self::TcpSocket, Self::Error> {
        Ok(TcpSocket::new())
    }

    fn receive(
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

    fn send(
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
        network: &mut Self::TcpSocket,
        remote: SocketAddr,
    ) -> nb::Result<(), Self::Error> {
        TcpStream::connect(format!("{}", remote))
            .map(|stream| drop(network.stream.replace(stream)))
            .map_err(|_| ().into())
    }

    fn close(&self, _network: Self::TcpSocket) -> Result<(), Self::Error> {
        Ok(())
    }
}
