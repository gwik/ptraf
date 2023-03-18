use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use clap::Parser;
use log::info;
use tokio::signal;

mod clock;
mod probe;
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
    /// Window duration.
    #[arg(short, long, default_value_t = 250u64)]
    window_ms: u64,
    /// Number of segments to keep.
    #[arg(short, long, default_value_t = 240usize)]
    segment_count: usize,

    /// Per core message buffer capacity.
    #[arg(long, default_value_t = unsafe { NonZeroUsize::new_unchecked(4096) })]
    msg_buffer_capacity: NonZeroUsize,

    /// Frequency of the display.
    #[arg(short, long, default_value_t = 1000)]
    ui_refresh_rate_ms: u64,
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();

    let args = Args::parse();

    let clock = ClockNano::default();
    let store = Store::new(Duration::from_millis(args.window_ms), args.segment_count);
    let app = Arc::new(App::new(clock, store));

    let ui_handle = {
        let app = Arc::clone(&app);
        tokio::spawn(run_ui(
            Arc::clone(&app),
            Duration::from_millis(args.ui_refresh_rate_ms),
        ))
    };

    let program = ProbeProgram::load()?;
    info!("BPF program loaded");
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
