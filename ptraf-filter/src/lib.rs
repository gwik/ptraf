use std::net::IpAddr;

mod frontend;
mod interpretor;

pub use frontend::*;
pub use interpretor::*;

pub trait Filterable {
    fn pid(&self) -> u32;

    fn protocol(&self) -> Protocol;

    fn ip_version(&self) -> IpVersion;

    fn local_address(&self) -> IpAddr;

    fn remote_address(&self) -> IpAddr;

    fn local_port(&self) -> u16;

    fn remote_port(&self) -> u16;
}
