use embedded_nal::{AddrType, Dns, IpAddr, SocketAddr, TcpClientStack};
use heapless::String;
use std::io::{Read, Write};
use std::net::TcpStream;

use dns_lookup::{lookup_addr, lookup_host};

/// An std::io::Error compatible error type returned when an operation is requested in the wrong
/// sequence (where the "right" is create a socket, connect, any receive/send, and possibly close).
#[derive(Debug)]
struct OutOfOrder;

impl std::fmt::Display for OutOfOrder {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Out of order operations requested")
    }
}

impl std::error::Error for OutOfOrder {}

impl<T> Into<std::io::Result<T>> for OutOfOrder {
    fn into(self) -> std::io::Result<T> {
        Err(std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            OutOfOrder,
        ))
    }
}

pub struct Network;

pub(crate) fn to_nb(e: std::io::Error) -> nb::Error<std::io::Error> {
    use std::io::ErrorKind::{TimedOut, WouldBlock};
    match e.kind() {
        WouldBlock | TimedOut => nb::Error::WouldBlock,
        _ => e.into(),
    }
}

pub struct TcpSocket {
    pub stream: Option<TcpStream>,
}

impl TcpSocket {
    pub fn new() -> Self {
        TcpSocket { stream: None }
    }

    pub fn get_running(&mut self) -> std::io::Result<&mut TcpStream> {
        match self.stream {
            Some(ref mut s) => Ok(s),
            _ => OutOfOrder.into(),
        }
    }
}

impl Dns for Network {
    type Error = ();

    fn get_host_by_address(&mut self, ip_addr: IpAddr) -> nb::Result<String<256>, Self::Error> {
        let ip: std::net::IpAddr = format!("{}", ip_addr).parse().unwrap();
        let host = lookup_addr(&ip).unwrap();
        Ok(String::from(host.as_str()))
    }
    fn get_host_by_name(
        &mut self,
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
    type Error = std::io::Error;
    type TcpSocket = TcpSocket;

    fn socket(&mut self) -> Result<Self::TcpSocket, Self::Error> {
        Ok(TcpSocket::new())
    }

    fn receive(
        &mut self,
        network: &mut Self::TcpSocket,
        buf: &mut [u8],
    ) -> nb::Result<usize, Self::Error> {
        let socket = network.get_running()?;
        socket.read(buf).map_err(to_nb)
    }

    fn send(
        &mut self,
        network: &mut Self::TcpSocket,
        buf: &[u8],
    ) -> nb::Result<usize, Self::Error> {
        let socket = network.get_running()?;
        socket.write(buf).map_err(to_nb)
    }

    fn is_connected(&mut self, network: &Self::TcpSocket) -> Result<bool, Self::Error> {
        Ok(network.stream.is_some())
    }

    fn connect(
        &mut self,
        network: &mut Self::TcpSocket,
        remote: SocketAddr,
    ) -> nb::Result<(), Self::Error> {
        let soc = TcpStream::connect(format!("{}", remote))?;
        soc.set_nonblocking(true)?;
        network.stream.replace(soc);
        Ok(())
    }

    fn close(&mut self, _network: Self::TcpSocket) -> Result<(), Self::Error> {
        // No-op: Socket gets closed when it is freed
        //
        // Could wrap it in an Option, but really that'll only make things messier; users will
        // probably drop the socket anyway after closing, and can't expect it to be usable with
        // this API.
        Ok(())
    }
}
