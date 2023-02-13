use crate::bindings::{in6_addr, in6_addr__bindgen_ty_1 as in6_u, sock_type};

/// Version tag for IPs.
#[derive(Copy, Clone)]
#[repr(u8)]
#[cfg_attr(feature = "user", derive(Debug))]
pub enum IpVersion {
    /// IPv4
    V4 = 0,
    /// IPv6
    V6 = 1,
}

/// Core types for Ip address.
///
/// `IpAddr` is not reprensented as a V4 + V6 enum because the
/// BPF compiler is too strict about the initialization and sees
/// part of the enum as uninitialized.
#[derive(Copy, Clone)]
#[repr(C, packed)]
pub struct IpAddr {
    /// Version of the ip address, IPv4 addresses are stored in the upper bytes of `addr.`.
    pub version: IpVersion,
    /// The address storage for both v4 and v6 IP addresses.
    pub addr: in6_addr,
}

impl IpAddr {
    /// Builds an [IpAddr] from a v4 address represented as a network endian `u32`.
    pub fn v4(addr: u32) -> Self {
        Self {
            version: IpVersion::V4,
            addr: in6_addr {
                in6_u: in6_u {
                    u6_addr32: [0, 0, 0, addr],
                },
            },
        }
    }

    /// Builds an [IpAddr] from a v6 address.
    pub fn v6(addr: in6_addr) -> Self {
        Self {
            version: IpVersion::V6,
            addr,
        }
    }
}

/// Builds an ip from the address
#[cfg(feature = "user")]
impl From<IpAddr> for std::net::IpAddr {
    fn from(ip: IpAddr) -> std::net::IpAddr {
        match ip.version {
            IpVersion::V4 => {
                let addr: std::net::Ipv4Addr =
                    u32::from_be(unsafe { ip.addr.in6_u.u6_addr32[3] }).into();
                addr.into()
            }
            IpVersion::V6 => {
                let a = unsafe { ip.addr.in6_u.u6_addr16 };
                std::net::Ipv6Addr::new(
                    u16::from_be(a[0]),
                    u16::from_be(a[1]),
                    u16::from_be(a[2]),
                    u16::from_be(a[3]),
                    u16::from_be(a[4]),
                    u16::from_be(a[5]),
                    u16::from_be(a[6]),
                    u16::from_be(a[7]),
                )
                .into()
            }
        }
    }
}

#[cfg(feature = "user")]
impl core::fmt::Debug for IpAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let ip: std::net::IpAddr = (*self).into();
        ip.fmt(f)
    }
}

/// Version tag for IPs.
///
///```no_run
/// SOCK_STREAM: Type = 1;
/// SOCK_DGRAM: Type = 2;
/// SOCK_RAW: Type = 3;
/// SOCK_RDM: Type = 4;
/// SOCK_SEQPACKET: Type = 5;
/// SOCK_DCCP: Type = 6;
/// SOCK_PACKET: Type = 10;
///```
#[derive(Copy, Clone)]
#[repr(transparent)]
pub struct SockType(sock_type::Type);

impl From<sock_type::Type> for SockType {
    fn from(val: sock_type::Type) -> Self {
        Self(val)
    }
}

#[cfg(feature = "user")]
impl std::fmt::Debug for SockType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self.0 {
            sock_type::SOCK_STREAM => "SOCK_STREAM",
            sock_type::SOCK_DGRAM => "SOCK_DGRAM",
            sock_type::SOCK_RAW => "SOCK_RAW",
            sock_type::SOCK_RDM => "SOCK_RDM",
            sock_type::SOCK_SEQPACKET => "SOCK_SEQPACKET",
            sock_type::SOCK_DCCP => "SOCK_DCCP",
            sock_type::SOCK_PACKET => "SOCK_PACKET",
            _ => "SOCK_UNKNOWN",
        };
        f.write_str(s)
    }
}

/// Event triggered on allocation of a sockfs inode for a socket.
#[repr(C, packed)]
#[derive(Copy, Clone)]
#[cfg_attr(feature = "user", derive(Debug))]
pub struct SockMsgEvent {
    /// Socket type
    pub sock_type: SockType,
    /// Local bound address.
    pub local_addr: IpAddr,
    /// Remote address.
    pub remote_addr: IpAddr,
    /// Source port (network endian).
    pub local_port: u16,
    /// Desination port (network endian).
    pub remote_port: u16,
    /// Length of the IP payload.
    pub len: u32,
    /// Process ID.
    pub pid: u32,
    /// Channel
    pub channel: Channel,
}

#[repr(u8)]
#[derive(Clone, Copy)]
#[cfg_attr(feature = "user", derive(Debug))]
pub enum Channel {
    Tx = 0,
    Rx = 1,
}

impl Channel {
    pub fn display(&self) -> &'static str {
        match self {
            Self::Tx => "TX",
            Self::Rx => "RX",
        }
    }
}
