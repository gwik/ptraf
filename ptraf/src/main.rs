use std::net::IpAddr;

use log::info;
use tokio::signal;

use ptraf::probe::ProbeProgram;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();

    let program = ProbeProgram::load()?;
    info!("BPF program loaded");
    let mut join_set = program
        .events(32.try_into()?, |events, cpu_id| {
            for event in events {
                let local_addr: IpAddr = event.local_addr.into();
                let remote_addr: IpAddr = event.remote_addr.into();
                let local_port = event.local_port;
                let remove_port = event.remote_port;
                let len = event.ret;
                let pid = event.pid;
                let sock_type = event.sock_type;
                let channel = event.channel;
                println!(
                    "[{pid}:{cpu_id}] {channel:?} {sock_type:?} {local_addr}:{local_port} -> {remote_addr}:{remove_port} {len}"
                );
            }
        })
        .await?;

    tokio::select! {
        _ = signal::ctrl_c() => {
            info!("Exiting...");
            join_set.abort_all();
            while join_set.join_next().await.is_some() {};
            Ok(())
        },
        res = join_set.join_next() => res.ok_or_else(|| anyhow::anyhow!("BPF task exited"))??,
    }
}
