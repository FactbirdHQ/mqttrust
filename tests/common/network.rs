use embedded_nal_async::{AddrType, Dns, IpAddr, SocketAddr};
use native_tls::{TlsConnector, TlsStream};
use std::io::{Read, Write};
use std::marker::PhantomData;
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

pub struct Network<T> {
    tls_connector: Option<(TlsConnector, String)>,
    _sec: PhantomData<T>,
}

impl Network<TlsStream<TcpStream>> {
    pub fn new_tls(tls_connector: TlsConnector, hostname: String) -> Self {
        Self {
            tls_connector: Some((tls_connector, hostname)),
            _sec: PhantomData,
        }
    }
}

impl Network<TcpStream> {
    pub fn new() -> Self {
        Self {
            tls_connector: None,
            _sec: PhantomData,
        }
    }
}

pub struct TcpSocket<T> {
    pub stream: Option<T>,
}

impl<T> TcpSocket<T> {
    pub fn new() -> Self {
        TcpSocket { stream: None }
    }

    pub fn get_running(&mut self) -> std::io::Result<&mut T> {
        match self.stream {
            Some(ref mut s) => Ok(s),
            _ => OutOfOrder.into(),
        }
    }
}


impl<T> Dns for Network<T> {
	type Error = ();

	async fn get_host_by_name(
		&self,
		host: &str,
		addr_type: AddrType,
	) -> Result<IpAddr, Self::Error> {
        todo!()
    }

	async fn get_host_by_address(&self, addr: IpAddr) -> Result<heapless::String<256>, Self::Error> {
        todo!()
    }
}