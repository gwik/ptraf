use std::net::IpAddr;

/// Protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
}

/// The IP version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpVersion {
    IpV4,
    IpV6,
}

/// The expression of the filter language.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Expr {
    Pid(u32),

    Protocol(Protocol),
    IpVersion(IpVersion),

    Addr(IpAddr),
    LocalAddr(IpAddr),
    RemoteAddr(IpAddr),

    Port(u16),
    LocalPort(u16),
    RemotePort(u16),

    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

peg::parser!(pub grammar parser() for str {

    pub rule filter() -> Expr
        = logic()

    rule operand() -> Expr
        = pid() / udp() / tcp() / ipv4() / ipv6() / ports() / addrs()

    rule pid() -> Expr
        = _ "pid[" n:$(['0'..='9']+) "]" _ {? n.parse::<u32>().or(Err("invalid pid number")).map(Expr::Pid) }

    rule udp() -> Expr
        = _ "udp" _ { Expr::Protocol(Protocol::Udp) }

    rule tcp() -> Expr
        = _ "tcp" _ { Expr::Protocol(Protocol::Tcp) }

    rule ipv4() -> Expr
        = _ "ipv4" _ { Expr::IpVersion(IpVersion::IpV4) }

    rule ipv6() -> Expr
        = _ "ipv6" _ { Expr::IpVersion(IpVersion::IpV6) }

    rule ports() -> Expr
        = port() / local_port() / remote_port()

    rule port() -> Expr
        = _ "port[" n:port_number() "]" _ { Expr::Port(n) }

    rule local_port() -> Expr
        = _ "lport[" n:port_number() "]" _ { Expr::LocalPort(n) }

    rule remote_port() -> Expr
        = _ "rport[" n:port_number() "]" _ { Expr::RemotePort(n) }

    rule port_number() -> u16
        = n:$(['0'..='9']+) {? n.parse::<u16>().or(Err("invalid port number")) }

    rule addrs() -> Expr
        = addr() / local_addr() / remote_addr()

    rule addr() -> Expr
        = _ "addr[" n:addr_any() "]" _ { Expr::Addr(n) }

    rule local_addr() -> Expr
        = _ "laddr[" n:addr_any() "]" _ { Expr::LocalAddr(n) }

    rule remote_addr() -> Expr
        = _ "raddr[" n:addr_any() "]" _ { Expr::RemoteAddr(n) }

    rule addr_any() -> IpAddr
        = n:$(['0'..='9' | 'a'..='f' | 'A'..='F' | ':' | '.' ]+) {? n.parse::<IpAddr>().or(Err("invalid ip address")).map(Into::into) }

    rule logic() -> Expr = precedence!{
      a:(@) _ "or" _ b:@ { Expr::Or(Box::new(a), Box::new(b)) }
      a:(@) _ "and" _ b:@ { Expr::And(Box::new(a), Box::new(b)) }
      --
      s: operand() { s }
      "(" _ e:logic() _ ")" { e }
    }

    rule _() =  quiet!{[' ' | '\t']*}
});

#[cfg(test)]
mod tests {

    use std::net::Ipv4Addr;

    use super::*;
    use pretty_assertions::assert_eq;

    use IpVersion::*;
    use Protocol::*;

    macro_rules! assert_parse {
        ($s: expr, $exp: expr) => {
            assert_eq!($crate::frontend::parser::filter($s), Ok($exp), $s);
        };
    }

    macro_rules! assert_error {
        ($s: expr, $col: expr) => {
            let err = $crate::frontend::parser::filter($s).unwrap_err();
            assert_eq!($col, err.location.column);
        };
    }

    #[test]
    fn single_statements() {
        assert_parse!("pid[1293]", Expr::Pid(1293));

        assert_parse!("udp", Expr::Protocol(Udp));
        assert_parse!("tcp", Expr::Protocol(Tcp));

        assert_error!("stcp", 1);
        assert_error!("uvp", 1);

        assert_parse!("ipv4", Expr::IpVersion(IpV4));
        assert_parse!("ipv6", Expr::IpVersion(IpV6));

        assert_parse!("port[1231]", Expr::Port(1231));
        assert_parse!("lport[2345]", Expr::LocalPort(2345));
        assert_parse!("rport[43323]", Expr::RemotePort(43323));

        assert_error!("port[1239921232]", 16);

        let ex_v6_addr = "1050:0:0:0:5:600:300c:326b".parse::<IpAddr>().unwrap();

        assert_parse!(
            "raddr[1050:0:0:0:5:600:300c:326b]",
            Expr::RemoteAddr(ex_v6_addr)
        );

        assert_parse!(
            "addr[10.0.254.0]",
            Expr::Addr(IpAddr::V4(Ipv4Addr::new(10, 0, 254, 0)))
        );
        assert_parse!(
            "laddr[10.0.234.4]",
            Expr::LocalAddr(IpAddr::V4(Ipv4Addr::new(10, 0, 234, 4)))
        );
        assert_parse!(
            "raddr[192.168.1.12]",
            Expr::RemoteAddr(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 12)))
        );
        assert_parse!(
            "raddr[1.1.1.1]",
            Expr::RemoteAddr(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
        );
    }

    #[test]
    fn logical_operators() {
        assert_parse!(
            "(pid[3221] or tcp) and ipv4",
            Expr::And(
                Box::new(Expr::Or(
                    Box::new(Expr::Pid(3221)),
                    Box::new(Expr::Protocol(Protocol::Tcp))
                )),
                Box::new(Expr::IpVersion(IpVersion::IpV4))
            )
        );

        assert_parse!(
            "pid[3221] or tcp and ipv4",
            Expr::And(
                Box::new(Expr::Or(
                    Box::new(Expr::Pid(3221)),
                    Box::new(Expr::Protocol(Protocol::Tcp))
                )),
                Box::new(Expr::IpVersion(IpVersion::IpV4))
            )
        );

        assert_parse!(
            "pid[3221] and tcp or ipv4",
            Expr::Or(
                Box::new(Expr::And(
                    Box::new(Expr::Pid(3221)),
                    Box::new(Expr::Protocol(Protocol::Tcp))
                )),
                Box::new(Expr::IpVersion(IpVersion::IpV4))
            )
        );
    }
}
