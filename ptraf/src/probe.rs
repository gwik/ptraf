//! This module provides an eBPF program for logging and analyzing send and receive events on sockets.
//!
//! The `ProbeProgram` struct contains the program itself, and provides a method for launching a task for
//! each CPU to read events from the kernel and pass them to a provided function.
//!
//! The `EventIter` struct is an iterator over references to `SockMsgEvent` structs.
//!
//! # Example
//!
//! ```no_run
//! use std::num::NonZeroUsize;
//!
//! use ptraf::probe::{ProbeProgram, EventIter};
//! use ptraf_common::types::SockMsgEvent;
//! use tokio::task::JoinSet;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), anyhow::Error> {
//! // Load the program into the kernel.
//! let program = ProbeProgram::load()?;
//!
//! // Define a function to process events.
//! fn process_events(events: EventIter<'_>, cpu_id: u32) {
//!     for event in events {
//!         // Process each event.
//!     }
//! }
//!
//! // Start a task for each CPU to read events and pass them to the function.
//! let buffer_size = NonZeroUsize::new(1024).unwrap();
//! let mut join_set = program.events(buffer_size, process_events).await?;
//!
//! // Wait for all tasks to complete.
//! while let Some(res) = join_set.join_next().await {
//!     let _ = res.unwrap();
//! }
//! # Ok(())
//! # }
//! ```

use std::iter::FusedIterator;
use std::num::NonZeroUsize;
use std::sync::Arc;

use aya::maps::perf::AsyncPerfEventArray;
use aya::programs::KProbe;
use aya::util::online_cpus;
use aya::{include_bytes_aligned, Bpf};
use aya_log::BpfLogger;
use bytes::BytesMut;
use log::{trace, warn};
use ptraf_common::types::SockMsgEvent;
use tokio::task::JoinSet;

/// The probing eBPF program.
pub struct ProbeProgram {
    bpf: Bpf,
}

impl ProbeProgram {
    /// Loads the program into the kernel and attaches different probes.
    pub fn load() -> Result<Self, anyhow::Error> {
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

        let probe: &mut KProbe = bpf.program_mut("msg").unwrap().try_into()?;
        probe.load()?;
        probe.attach("sock_sendmsg", 0)?;
        probe.attach("sock_recvmsg", 0)?;

        let ret_probe: &mut KProbe = bpf.program_mut("sendmsg_ret").unwrap().try_into()?;
        ret_probe.load()?;
        ret_probe.attach("sock_sendmsg", 0)?;

        let ret_probe: &mut KProbe = bpf.program_mut("recvmsg_ret").unwrap().try_into()?;
        ret_probe.load()?;
        ret_probe.attach("sock_recvmsg", 0)?;

        trace!("probe program loaded");

        Ok(Self { bpf })
    }

    /// Consumes `self` and launches one task per CPU, each of which reads events from
    /// the kernel and passes them in a batch through the provided function `f`. The function returns
    /// a `JoinSet` which can wait for all tasks to complete.
    ///
    /// # Arguments
    ///
    /// * `buffer_size`: Size of the, per task, buffer for reading events.
    /// * `f`: The function that will be called with an `EventIter` and the ID of the CPU that produced the events.
    ///
    /// # Returns
    ///
    /// A `Result` that either contains a `JoinSet` that can wait for all tasks to complete or an `anyhow::Error`
    /// if there was an error while launching tasks.
    pub async fn events<F>(
        self,
        buffer_size: NonZeroUsize,
        f: F,
    ) -> Result<JoinSet<Result<(), anyhow::Error>>, anyhow::Error>
    where
        F: Fn(EventIter<'_>, u32) + Send + Sync + 'static,
    {
        let mut join_set = JoinSet::new();
        let f = Arc::new(f);

        // Create an `AsyncPerfEventArray` for reading events.
        let mut perf_array = AsyncPerfEventArray::try_from(self.bpf.map_mut("EVENTS")?)?;

        // Create an Arc of the bpf program so that each task retains it.
        let bpf = Arc::new(self.bpf);

        trace!("spawning per cpu tasks");

        // Iterate over each online CPU and spawn a task for each.
        for cpu_id in online_cpus()? {
            // Open a separate perf buffer for each CPU.
            let mut buf = perf_array.open(cpu_id, Some(4096))?;
            let f = Arc::clone(&f);
            let bpf = Arc::clone(&bpf);

            // Process each perf buffer in a separate task.
            join_set.spawn(async move {
                let _bpf = bpf;
                let f = &*f;
                // Create a buffer to store events for the task.
                let mut buffers = (0..buffer_size.into())
                    .map(|_| BytesMut::with_capacity(std::mem::size_of::<SockMsgEvent>()))
                    .collect::<Vec<_>>();

                trace!("waiting for events cpu={}", cpu_id);

                loop {
                    // Wait for events.
                    let events = buf.read_events(buffers.as_mut_slice()).await?;
                    let event_buf = EventIter::new(&buffers[0..events.read]);
                    trace!(
                        "run events callback cpu={} read={} lost={}",
                        cpu_id,
                        events.read,
                        events.lost
                    );
                    f(event_buf, cpu_id);
                }
            });
        }

        // Return a join set to wait for all tasks to complete.
        Ok::<_, anyhow::Error>(join_set)
    }
}

/// An iterator over [SockMsgEvent] references.
pub struct EventIter<'a> {
    buf: &'a [BytesMut],
    cur: usize,
}

impl<'a> EventIter<'a> {
    fn new(buf: &'a [BytesMut]) -> Self {
        Self { cur: 0, buf }
    }
}

impl<'a> Iterator for EventIter<'a> {
    type Item = &'a SockMsgEvent;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cur >= self.buf.len() {
            return None;
        }

        // SAFETY: This EventIter is always created from buffers that contains [SockMsgEvent].
        let msg: &SockMsgEvent = unsafe { &*(self.buf[self.cur].as_ptr() as *const SockMsgEvent) };
        self.cur += 1;

        Some(msg)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.cur >= self.buf.len() {
            (0, Some(0))
        } else {
            let rem = self.buf.len() - self.cur;
            (rem, Some(rem))
        }
    }
}

impl ExactSizeIterator for EventIter<'_> {}
impl FusedIterator for EventIter<'_> {}
