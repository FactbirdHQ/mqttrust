use core::convert::TryFrom;
use core::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use embedded_nal_async::{AddrType, Dns};

/// Default MQTT port without TLS.
#[cfg(not(feature = "embedded-tls"))]
const MQTT_DEFAULT_PORT: u16 = 1883;

/// Default MQTT port with TLS.
#[cfg(feature = "embedded-tls")]
const MQTT_DEFAULT_PORT: u16 = 8883;

/// Represents an MQTT broker.
///
/// This trait provides a common interface to retrieve the broker's address,
/// which might involve resolving a domain name.
pub trait Broker {
    /// Retrieves the broker's `SocketAddr`.
    ///
    /// This method should attempt to resolve the address if it's not readily available (e.g.,
    /// for domain-based brokers).
    async fn get_address(&mut self) -> Option<SocketAddr>;

    /// Retrieves the hostname of the broker, if applicable.
    ///
    /// This method returns `None` for IP-based brokers.
    fn get_hostname(&self) -> Option<&str> {
        None
    }
}

/// Represents an MQTT broker specified by a domain name.
///
/// The domain name is resolved to an IP address when `get_address` is called.
#[derive(Debug)]
pub struct DomainBroker<'a, R: Dns, const T: usize = 253> {
    /// The raw domain name string of the broker.
    raw: heapless::String<T>,

    /// A reference to the DNS resolver used to resolve the domain name.
    resolver: &'a R,

    /// The resolved `SocketAddr` of the broker.
    addr: SocketAddr,
}

impl<'a, R: Dns, const T: usize> DomainBroker<'a, R, T> {
    /// Creates a new `DomainBroker`.
    ///
    /// # Arguments
    /// * `broker` - The domain name of the broker (e.g., `broker.example.com` or `broker.example.com:8883`).
    /// * `resolver` - A `embedded_nal::Dns` resolver for resolving the domain name.
    ///
    /// # Errors
    /// Returns an error if the provided `broker` string cannot be parsed or is too long.
    pub fn new(broker: &str, resolver: &'a R) -> Result<Self, ()> {
        // Attempt to parse the broker string as a SocketAddr.
        // If parsing fails, it creates a SocketAddr with an unspecified IP and the provided or default port.
        let addr: SocketAddr = broker.parse().unwrap_or_else(|_| {
            let (_, port) = broker
                .split_once(':')
                .map(|(b, port)| (b, port.parse().unwrap_or(MQTT_DEFAULT_PORT)))
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
    /// Resolves and returns the `SocketAddr` of the broker.
    ///
    /// If the IP address is unspecified, it attempts to resolve the domain name using the provided resolver.
    async fn get_address(&mut self) -> Option<SocketAddr> {
        // If the IP address is unspecified, it needs to be resolved
        if self.addr.ip().is_unspecified() {
            let hostname = self
                .raw
                .split_once(':')
                .map(|f| f.0)
                .unwrap_or(self.raw.as_str());

            info!("Trying to resolve IP of {:?}", hostname);

            // Try resolving the hostname
            match self
                .resolver
                .get_host_by_name(hostname, AddrType::IPv4)
                .await
            {
                Ok(ip) => self.addr.set_ip(ip), // Update the address with the resolved IP
                Err(_e) => {
                    #[cfg(feature = "log")]
                    warn!("DNS lookup failed: {:?}", _e);
                    #[cfg(feature = "defmt")]
                    warn!("DNS lookup failed: {:?}", defmt::Debug2Format(&_e));
                }
            }
        }

        // Return the SocketAddr if the IP is now specified, otherwise return None
        if !self.addr.ip().is_unspecified() {
            Some(self.addr)
        } else {
            None
        }
    }

    fn get_hostname(&self) -> Option<&str> {
        // Extract and return the hostname from the 'raw' string
        let hostname = self
            .raw
            .split_once(':')
            .map(|f| f.0)
            .unwrap_or(self.raw.as_str());

        Some(hostname)
    }
}

/// Represents an MQTT broker with a known IP address.
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct IpBroker {
    /// The `SocketAddr` of the broker.
    addr: SocketAddr,
}

impl IpBroker {
    /// Creates a new `IpBroker` with the specified IP address and port.
    pub fn new(broker: impl Into<IpAddr>, port: u16) -> Self {
        Self {
            addr: SocketAddr::new(broker.into(), port),
        }
    }
}

impl Broker for IpBroker {
    /// Returns the stored `SocketAddr` of the broker.
    async fn get_address(&mut self) -> Option<SocketAddr> {
        Some(self.addr)
    }
}

// Implement From trait for various types to simplify creating an IpBroker
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
