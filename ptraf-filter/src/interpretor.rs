use peg::{error::ParseError, str::LineCol};

use crate::{
    frontend::{parser, Expr},
    Filterable,
};

pub struct Interpretor {
    ast: Expr,
}

impl Interpretor {
    pub fn parse(input: &str) -> Result<Self, ParseError<LineCol>> {
        parser::filter(input).map(|expr| Self { ast: expr })
    }

    pub fn new(ast: Expr) -> Self {
        Self { ast }
    }

    pub fn filter<F: Filterable>(&self, f: &F) -> bool {
        Self::eval(f, &self.ast)
    }

    fn eval<F: Filterable>(f: &F, o: &Expr) -> bool {
        match o {
            Expr::Pid(pid) => f.pid() == *pid,
            Expr::Protocol(p) => f.protocol() == *p,
            Expr::IpVersion(v) => f.ip_version() == *v,
            Expr::Addr(addr) => &f.local_address() == addr || &f.remote_address() == addr,
            Expr::LocalAddr(addr) => &f.local_address() == addr,
            Expr::RemoteAddr(addr) => &f.remote_address() == addr,
            Expr::Port(p) => &f.local_port() == p || &f.remote_port() == p,
            Expr::LocalPort(p) => &f.local_port() == p,
            Expr::RemotePort(p) => &f.remote_port() == p,
            Expr::And(a, b) => Self::and(f, a, b),
            Expr::Or(a, b) => Self::or(f, a, b),
            Expr::Not(a) => Self::not(f, a),
        }
    }

    #[inline]
    fn or<F: Filterable>(f: &F, a: &Expr, b: &Expr) -> bool {
        Self::eval(f, a) || Self::eval(f, b)
    }

    #[inline]
    fn and<F: Filterable>(f: &F, a: &Expr, b: &Expr) -> bool {
        Self::eval(f, a) && Self::eval(f, b)
    }

    #[inline]
    fn not<F: Filterable>(f: &F, a: &Expr) -> bool {
        !Self::eval(f, a)
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use crate::frontend::{IpVersion, Protocol};

    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Packet {
        pid: u32,
        protocol: Protocol,
        ip_version: IpVersion,
        local_address: IpAddr,
        remote_address: IpAddr,
        local_port: u16,
        remote_port: u16,
    }

    impl Filterable for Packet {
        fn pid(&self) -> u32 {
            self.pid
        }

        fn protocol(&self) -> Protocol {
            self.protocol
        }

        fn ip_version(&self) -> IpVersion {
            self.ip_version
        }

        fn local_address(&self) -> IpAddr {
            self.local_address
        }

        fn remote_address(&self) -> IpAddr {
            self.remote_address
        }

        fn local_port(&self) -> u16 {
            self.local_port
        }

        fn remote_port(&self) -> u16 {
            self.remote_port
        }
    }

    #[test]
    fn filtering() {
        let packet0 = Packet {
            pid: 213,
            protocol: Protocol::Tcp,
            ip_version: IpVersion::IpV4,
            local_address: Ipv4Addr::new(127, 0, 0, 1).into(),
            remote_address: Ipv4Addr::new(1, 1, 1, 1).into(),
            local_port: 12382,
            remote_port: 443,
        };

        let packet1 = Packet {
            pid: 213,
            protocol: Protocol::Tcp,
            ip_version: IpVersion::IpV4,
            local_address: Ipv4Addr::new(127, 0, 0, 1).into(),
            remote_address: Ipv4Addr::new(1, 1, 1, 1).into(),
            local_port: 12382,
            remote_port: 8443,
        };

        let interpretor =
            Interpretor::parse("tcp and (laddr[127.0.0.1] or laddr[192.168.1.32]) and rport[443]")
                .unwrap();

        assert!(interpretor.filter(&packet0));
        assert!(!interpretor.filter(&packet1));
    }
}
