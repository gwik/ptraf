use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use clap::Parser;
use log::info;
use tokio::signal;

use ptraf::{clock::ClockNano, probe::ProbeProgram, store::Store};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Window duration.
    #[arg(short, long, default_value_t = 250u64)]
    window_ms: u64,

    /// Per core message buffer capacity.
    #[arg(long, default_value_t = unsafe { NonZeroUsize::new_unchecked(4096) })]
    msg_buffer_capacity: NonZeroUsize,

    /// Frequency of the display.
    #[arg(short, long, default_value_t = 1000)]
    freq_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();

    let args = Args::parse();

    let clock = Arc::new(ClockNano::default());
    let store = Arc::new(Store::new(Duration::from_millis(args.window_ms), 240));

    {
        let clock = Arc::clone(&clock);
        let store = Arc::clone(&store);

        tokio::spawn(async move {
            let clock = Arc::clone(&clock);
            let mut now = clock.now();
            let freq = Duration::from_millis(args.freq_ms);
            loop {
                tokio::time::sleep(freq).await;
                let view = store.segments_view();

                if view.is_empty() {
                    continue;
                }

                let cur = clock.now();
                let (min_ts, val, packets) = view
                    .iter()
                    .rev()
                    .take_while(|segment| {
                        segment.ts.saturating_elapsed_since(&now)
                            < freq + Duration::from_millis(250)
                    })
                    .fold((cur, 0u64, 0u64), |(_, val, packets), segment| {
                        (
                            segment.ts,
                            val + segment.segment.total(None),
                            packets + segment.segment.total_packet_count(),
                        )
                    });

                let elapsed = min_ts.saturating_elapsed_since(&cur);

                let rate = if elapsed.is_zero() {
                    0u64
                } else {
                    (val as f64 / elapsed.as_secs_f64()) as u64
                };

                let packets = if elapsed.is_zero() {
                    0u64
                } else {
                    (packets as f64 / elapsed.as_secs_f64()) as u64
                };

                println!(
                    "segments={} rate={}/s packets={}/s elapsed={:?}",
                    view.len(),
                    humansize::format_size(rate, humansize::DECIMAL),
                    packets,
                    elapsed,
                );

                now = cur;
            }
        });
    }

    let program = ProbeProgram::load()?;
    info!("BPF program loaded");
    let mut join_set = program
        .events(args.msg_buffer_capacity, move |events, _cpu_id| {
            let ts = clock.now();

            store.batch_update(ts, events);

            // std::hint::black_box((ts, events));

            // for event in events {
            //     let local_addr: IpAddr = event.local_addr.into();
            //     let remote_addr: IpAddr = event.remote_addr.into();
            //     let local_port = event.local_port;
            //     let remove_port = event.remote_port;
            //     let len = event.ret;
            //     let pid = event.pid;
            //     let sock_type = event.sock_type;
            //     let channel = event.channel;
            //     println!(
            //         "[{pid}:{cpu_id}] {channel:?} {sock_type:?} {local_addr}:{local_port} -> {remote_addr}:{remove_port} {len}"
            //     );
            // }
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
