use embedded_nal::{Mode, SocketAddr, TcpStack};
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
