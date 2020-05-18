use embedded_nal::{AddrType, Dns, Mode, SocketAddr, TcpStack};
use heapless::{consts, String};
use no_std_net::IpAddr;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;

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

    fn gethostbyaddr(&self, _ip_addr: IpAddr) -> Result<String<consts::U256>, Self::Error> {
        unimplemented!()
    }
    fn gethostbyname(&self, _hostname: &str, _addr_type: AddrType) -> Result<IpAddr, Self::Error> {
        unimplemented!()
    }
}

impl TcpStack for Network {
    type Error = ();
    type TcpSocket = TcpSocket;

    fn open(&self, mode: Mode) -> Result<Self::TcpSocket, Self::Error> {
        Ok(TcpSocket::new(mode))
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
