#![no_std]
#![no_main]

use core::ffi::c_int;

use aya_bpf::helpers::{bpf_get_current_pid_tgid, bpf_probe_read_kernel};
use aya_bpf::maps::{HashMap, PerfEventArray};
use aya_bpf::BpfContext;
use aya_bpf::{
    macros::{kprobe, kretprobe, map},
    programs::ProbeContext,
};
// use aya_log_ebpf::debug;

use ptraf_common::types::{Channel, IpAddr, SockMsgEvent};

#[allow(non_upper_case_globals)]
#[allow(non_snake_case)]
#[allow(non_camel_case_types)]
#[allow(dead_code)]
#[allow(clippy::useless_transmute)]
#[allow(clippy::transmute_int_to_bool)]
#[allow(clippy::too_many_arguments)]
#[allow(clippy::unnecessary_cast)]
#[allow(clippy::type_complexity)]
#[allow(clippy::wrong_self_convention)]
mod bindings;

use bindings::{sock_common as SockCommon, socket as Socket};

// Force aya_log_epbf to be linked.
const _UNUSED: usize = aya_log_ebpf::LOG_BUF_CAPACITY;

// https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/net/socket.c

/// Shared events with the userland program.
#[map]
static mut EVENTS: PerfEventArray<SockMsgEvent> = PerfEventArray::new(0);

/// Internal temporary cache to store the socket between the probe and the return probe.
#[map]
static mut CACHE: HashMap<u64, *const Socket> = HashMap::with_max_entries(16384, 0);

/// Probe for sock_sendmsg and sock_recvmsg.
#[kprobe(name = "msg")]
pub fn msg(ctx: ProbeContext) -> u32 {
    unsafe { try_msg(ctx) }.unwrap_or(1)
}

/// Return probe for sock_sendmsg.
#[kretprobe(name = "recvmsg_ret")]
pub fn recv_msg_ret(ctx: ProbeContext) -> u32 {
    unsafe { try_msg_ret(ctx, Channel::Rx) }.unwrap_or(1)
}

/// Return probe for sock_recvmsg.
#[kretprobe(name = "sendmsg_ret")]
pub fn send_msg_ret(ctx: ProbeContext) -> u32 {
    unsafe { try_msg_ret(ctx, Channel::Tx) }.unwrap_or(1)
}

const AF_INET: u16 = 2;
const AF_INET6: u16 = 10;

unsafe fn notify(
    ctx: ProbeContext,
    socket: *const Socket,
    ret: c_int,
    channel: Channel,
) -> Result<(), i64> {
    let sk = bpf_probe_read_kernel(&(*socket).sk)?;
    let sk_common = bpf_probe_read_kernel(&(*sk).__sk_common as *const SockCommon)?;
    let sk_type = bpf_probe_read_kernel(&(*sk).sk_type)?;

    let (local_port, remote_port) = {
        let ports = sk_common.__bindgen_anon_3.skc_portpair;
        let local_port = (ports >> 16) as u16;
        let remote_port = ports as u16;

        (local_port, remote_port)
    };

    let (local_addr, remote_addr) = match sk_common.skc_family {
        AF_INET => {
            let local_addr = IpAddr::v4(sk_common.__bindgen_anon_1.__bindgen_anon_1.skc_rcv_saddr);
            let remote_addr = IpAddr::v4(sk_common.__bindgen_anon_1.__bindgen_anon_1.skc_daddr);

            // debug!(
            //     &ctx,
            //     "AF_INET6 src addr: {:ipv4}:{}, dest addr: {:ipv4}:{} pid: {} sk_type: {} len: {} channel: {}",
            //     u32::from_be(sk_common.__bindgen_anon_1.__bindgen_anon_1.skc_rcv_saddr),
            //     u16::from_be(local_port),
            //     u32::from_be(sk_common.__bindgen_anon_1.__bindgen_anon_1.skc_daddr),
            //     u16::from_be(remote_port),
            //     ctx.pid(),
            //     sk_type,
            //     len,
            //     channel.display(),
            // );

            (local_addr, remote_addr)
        }
        AF_INET6 => {
            // let src_addr = sk_common.skc_v6_rcv_saddr;
            // let dest_addr = sk_common.skc_v6_daddr;

            // debug!(
            //     &ctx,
            //     "AF_INET6 src addr: {:ipv6}:{}, dest addr: {:ipv6}:{} pid: {} sk_type: {} len: {} channel: {}",
            //     src_addr.in6_u.u6_addr8,
            //                     u16::from_be(local_port),
            //     dest_addr.in6_u.u6_addr8,
            //                 u16::from_be(remote_port),
            //     ctx.pid(),
            //     sk_type,
            //     len,
            //     channel.display(),
            // );

            let local_addr = IpAddr::v6(sk_common.skc_v6_rcv_saddr.in6_u.u6_addr16);
            let remote_addr = IpAddr::v6(sk_common.skc_v6_daddr.in6_u.u6_addr16);

            (local_addr, remote_addr)
        }
        _ => return Ok(()),
    };

    let event = SockMsgEvent {
        sock_type: sk_type.into(),
        pid: ctx.pid(),
        local_addr,
        remote_addr,
        ret,
        local_port,
        remote_port,
        channel,
    };

    EVENTS.output(&ctx, &event, 0);

    Ok(())
}

unsafe fn try_msg_ret(ctx: ProbeContext, channel: Channel) -> Result<u32, u32> {
    let pid_tgid = bpf_get_current_pid_tgid();
    let socket = if let Some(socket) = CACHE.get(&pid_tgid) {
        let _ = CACHE.remove(&pid_tgid);
        *socket
    } else {
        return Ok(0);
    };

    let val: c_int = ctx.ret().ok_or(1u32)?;

    match notify(ctx, socket, val, channel) {
        Ok(_) => Ok(0),
        Err(_) => Err(1),
    }
}

unsafe fn try_msg(ctx: ProbeContext) -> Result<u32, i64> {
    let socket: *const Socket = ctx.arg(0).ok_or(1i64)?;
    let sk = bpf_probe_read_kernel(&(*socket).sk)?;
    let sk_common = bpf_probe_read_kernel(&(*sk).__sk_common as *const SockCommon)?;

    if matches!(sk_common.skc_family, AF_INET | AF_INET6) {
        let pid_tgid = bpf_get_current_pid_tgid();
        CACHE.insert(&pid_tgid, &socket, 0)?;
    }
    Ok(0)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
