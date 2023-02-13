use core::ffi::c_int;

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
    /// Length of the IP payload. If negative contains and error `-errno`.
    pub ret: c_int,
    /// Process ID.
    pub pid: u32,
    /// Channel
    pub channel: Channel,
}

impl SockMsgEvent {
    /// Returns `Ok(size)` if the probed call was successful or `Err(errno)`.
    pub fn packet_size(&self) -> Result<u32, i32> {
        if self.ret >= 0 {
            Ok(self.ret as u32)
        } else {
            Err(-self.ret)
        }
    }
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

/// Version tag for IPs.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
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
    addr: [u16; 8],
    version: IpVersion,
}

impl IpAddr {
    /// Builds an [IpAddr] from a v4 address represented as a network endian `u32`.
    pub fn v4(addr: u32) -> Self {
        Self {
            version: IpVersion::V4,
            addr: [0, 0, 0, 0, 0, 0, (addr >> 16) as u16, addr as u16],
        }
    }

    /// Builds an [IpAddr] from a v6 address as an array of network endian `u16`.
    pub fn v6(addr: [u16; 8]) -> Self {
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
                    u32::from_be(((ip.addr[6] as u32) << 16) | (ip.addr[7] as u32)).into();
                addr.into()
            }
            IpVersion::V6 => std::net::Ipv6Addr::new(
                u16::from_be(ip.addr[0]),
                u16::from_be(ip.addr[1]),
                u16::from_be(ip.addr[2]),
                u16::from_be(ip.addr[3]),
                u16::from_be(ip.addr[4]),
                u16::from_be(ip.addr[5]),
                u16::from_be(ip.addr[6]),
                u16::from_be(ip.addr[7]),
            )
            .into(),
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

#[repr(u16)]
#[non_exhaustive]
#[derive(Debug, Copy, Clone)]
pub enum SockType {
    Unknown = 0,
    Stream = 1,
    Dgram = 2,
    Raw = 3,
    Rdm = 4,
    Seqpacket = 5,
    Dccp = 6,
    Packet = 10,
}

impl From<u16> for SockType {
    #[inline]
    fn from(val: u16) -> Self {
        match val {
            1 => Self::Stream,
            2 => Self::Dgram,
            3 => Self::Raw,
            4 => Self::Rdm,
            5 => Self::Seqpacket,
            6 => Self::Dccp,
            10 => Self::Packet,
            _ => Self::Unknown,
        }
    }
}

mod tests {
    #[cfg(feature = "user")]
    mod user {
        #[allow(unused)]
        use super::super::SockType;

        #[test]
        fn sock_type_from_u16() {
            let sock_type: SockType = 12301u16.into();
            eprintln!("sock type undefined: {:?}", sock_type);
        }
    }
}
