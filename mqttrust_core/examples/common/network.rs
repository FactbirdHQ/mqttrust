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

    fn get_host_by_address(&self, ip_addr: IpAddr) -> nb::Result<String<256>, Self::Error> {
        let ip: std::net::IpAddr = format!("{}", ip_addr).parse().unwrap();
        let host = lookup_addr(&ip).unwrap();
        Ok(String::from(host.as_str()))
    }
    fn get_host_by_name(
        &self,
        hostname: &str,
        _addr_type: AddrType,
    ) -> nb::Result<IpAddr, Self::Error> {
        let mut ips: Vec<std::net::IpAddr> = lookup_host(hostname).unwrap();
        format!("{}", ips.pop().unwrap())
            .parse()
            .map_err(|_| nb::Error::Other(()))
    }
}

impl TcpClientStack for Network {
    type Error = ();
    type TcpSocket = TcpSocket;

    fn socket(&mut self) -> Result<Self::TcpSocket, Self::Error> {
        Ok(TcpSocket::new())
    }

    fn receive(
        &mut self,
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
        &mut self,
        network: &mut Self::TcpSocket,
        buf: &[u8],
    ) -> Result<usize, nb::Error<Self::Error>> {
        if let Some(ref mut stream) = network.stream {
            Ok(stream.write(buf).map_err(|_| nb::Error::Other(()))?)
        } else {
            Err(nb::Error::Other(()))
        }
    }

    fn is_connected(&mut self, _network: &Self::TcpSocket) -> Result<bool, Self::Error> {
        Ok(true)
    }

    fn connect(
        &mut self,
        network: &mut Self::TcpSocket,
        remote: SocketAddr,
    ) -> nb::Result<(), Self::Error> {
        TcpStream::connect(format!("{}", remote))
            .map(|stream| drop(network.stream.replace(stream)))
            .map_err(|_| ().into())
    }

    fn close(&mut self, _network: Self::TcpSocket) -> Result<(), Self::Error> {
        Ok(())
    }
}
