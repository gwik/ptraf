use aya::{include_bytes_aligned, programs::TracePoint, Bpf};
use aya_log::BpfLogger;
use log::{info, trace, warn};
use tokio::signal;

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();

    #[cfg(debug_assertions)]
    let mut bpf = Bpf::load(include_bytes_aligned!(
        "../../target/bpfel-unknown-none/debug/ptraf"
    ))?;
    #[cfg(not(debug_assertions))]
    let mut bpf = Bpf::load(include_bytes_aligned!(
        "../../target/bpfel-unknown-none/release/ptraf"
    ))?;

    if let Err(e) = BpfLogger::init(&mut bpf) {
        // This can happen if you remove all log statements from your eBPF program.
        warn!("failed to initialize eBPF logger: {}", e);
    }

    // echo "1" | sudo tee /sys/kernel/debug/tracing/events/sock/inet_sock_set_state/enable

    let probe: &mut TracePoint = bpf.program_mut("sock_set_state").unwrap().try_into()?;
    probe.load()?;
    probe.attach("sock", "inet_sock_set_state")?;

    trace!("probe program loaded");

    info!("Waiting for Ctrl-C...");
    signal::ctrl_c().await?;
    info!("Exiting...");

    Ok(())
}
