use core::convert::TryFrom;
use embedded_nal_async::{AddrType, Dns, IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};

const MQTT_DEFAULT_PORT: u16 = 1883;

/// A type that allows us to (eventually) determine the broker address.
pub trait Broker {
    /// Retrieve the broker address (if available).
    async fn get_address(&mut self) -> Option<SocketAddr>;
}

/// A broker that is specified using a qualified domain-name. The name will be resolved at some
/// point in the future.
#[derive(Debug)]
pub struct DomainBroker<'a, R: Dns, const T: usize = 253> {
    raw: heapless::String<T>,
    resolver: &'a R,
    addr: SocketAddr,
}

impl<'a, R: Dns, const T: usize> DomainBroker<'a, R, T> {
    /// Construct a new domain broker.
    ///
    /// # Args
    /// * `broker` - The domain name of the broker, such as `broker.example.com`
    ///   or `broker.example.com:8883`
    /// * `resolver` - A [embedded_nal::Dns] resolver to resolve the broker
    /// domain name to an IP address.
    pub fn new(broker: &str, resolver: &'a R) -> Result<Self, ()> {
        let addr: SocketAddr = broker.parse().unwrap_or_else(|_| {
            let (_, port) = broker
                .split_once(':')
                .map(|(b, port)| (b, port.parse().unwrap()))
                .unwrap_or((broker, MQTT_DEFAULT_PORT));
            SocketAddr::new(
                IpAddr::V4(broker.parse().unwrap_or(Ipv4Addr::UNSPECIFIED)),
                port,
            )
        });

        Ok(Self {
            raw: heapless::String::try_from(broker).map_err(drop)?,
            resolver,
            addr,
        })
    }
}

impl<'a, R: Dns, const T: usize> Broker for DomainBroker<'a, R, T> {
    async fn get_address(&mut self) -> Option<SocketAddr> {
        // Attempt to resolve the address.
        if self.addr.ip().is_unspecified() {
            match self
                .resolver
                .get_host_by_name(&self.raw, AddrType::IPv4)
                .await
            {
                Ok(ip) => self.addr.set_ip(ip),
                Err(_e) => {
                    #[cfg(feature = "log")]
                    warn!("DNS lookup failed: {:?}", _e);
                    #[cfg(feature = "defmt")]
                    warn!("DNS lookup failed: {:?}", defmt::Debug2Format(&_e));
                }
            }
        }

        if !self.addr.ip().is_unspecified() {
            Some(self.addr)
        } else {
            None
        }
    }
}

/// A simple broker specification where the address of the broker is known in advance.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct IpBroker {
    addr: SocketAddr,
}

impl IpBroker {
    /// Construct a broker with a known IP address & port.
    pub fn new(broker: impl Into<IpAddr>, port: u16) -> Self {
        Self {
            addr: SocketAddr::new(broker.into(), port),
        }
    }
}
impl Broker for IpBroker {
    async fn get_address(&mut self) -> Option<SocketAddr> {
        Some(self.addr)
    }
}

impl From<(IpAddr, u16)> for IpBroker {
    fn from((addr, port): (IpAddr, u16)) -> Self {
        IpBroker::new(addr, port)
    }
}

impl From<SocketAddr> for IpBroker {
    fn from(addr: SocketAddr) -> Self {
        IpBroker::new(addr.ip(), addr.port())
    }
}

impl From<SocketAddrV4> for IpBroker {
    fn from(addr: SocketAddrV4) -> Self {
        IpBroker::new(*addr.ip(), addr.port())
    }
}

impl From<(Ipv4Addr, u16)> for IpBroker {
    fn from((addr, port): (Ipv4Addr, u16)) -> Self {
        IpBroker::new(addr, port)
    }
}
