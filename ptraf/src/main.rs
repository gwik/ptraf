use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use clap::Parser;
use log::info;
use tokio::signal;

mod clock;
mod probe;
mod promise;
mod store;
mod ui;

use self::{
    clock::ClockNano,
    probe::ProbeProgram,
    store::Store,
    ui::{run_ui, App},
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Number of seconds of history to store.
    /// Defaults to 30s.
    #[arg(short, long, default_value_t = 30u64)]
    backlog_secs: u64,

    /// Per core message buffer capacity.
    #[arg(long, default_value_t = { NonZeroUsize::new(4096).unwrap() })]
    msg_buffer_capacity: NonZeroUsize,

    /// Frequency of the display.
    #[arg(short, long, default_value_t = 500)]
    ui_refresh_rate_ms: u64,

    /// Duration of a unit of storage in milliseconds. min: 10ms.
    #[arg(short, long, default_value_t = 250u64)]
    interval_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();

    let args = Args::parse();

    let clock = ClockNano::default();

    let segment_interval = Duration::from_millis(args.interval_ms.max(10));
    let segment_count = (args.backlog_secs * 1000 / (args.interval_ms.max(10))).max(1) as usize;

    let store = Store::new(segment_interval, segment_count);
    let app = Arc::new(App::new(clock, store));

    let program = ProbeProgram::load()?;
    info!("BPF program loaded");

    let ui_handle = {
        let app = Arc::clone(&app);
        tokio::spawn(run_ui(
            Arc::clone(&app),
            Duration::from_millis(args.ui_refresh_rate_ms),
        ))
    };

    let mut join_set = program
        .events(args.msg_buffer_capacity, move |events, _cpu_id| {
            let ts = app.clock().now();

            app.store().batch_update(ts, events);
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
        ui_res = ui_handle => { ui_res? },
    }
}
