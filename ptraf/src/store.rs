use std::{
    collections::VecDeque,
    hash::Hash,
    net::{IpAddr, SocketAddr},
    ops::Deref,
    sync::{
        atomic::{AtomicU64, Ordering},
        RwLock, RwLockReadGuard, TryLockError,
    },
    time::Duration,
};

use dashmap::{DashMap, DashSet};
use fxhash::FxBuildHasher;
use ptraf_common::{Channel, SockMsgEvent, SockType};

use crate::clock::Timestamp;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Interest {
    RemoteIp(IpAddr),
    RemoteSocket(SocketAddr),
    LocalSocket(SocketAddr),
    Pid(u32),
}

#[derive(Debug, Default)]
pub struct Stat {
    rx: AtomicU64,
    tx: AtomicU64,
    packet_count: AtomicU64,
}

impl Stat {
    #[inline]
    pub fn with_channel(&self, channel: Channel) -> &AtomicU64 {
        match channel {
            Channel::Rx => &self.rx,
            Channel::Tx => &self.tx,
        }
    }

    fn add(&self, channel: Channel, val: u64) {
        self.packet_count.fetch_add(1, Ordering::Relaxed);
        self.with_channel(channel).fetch_add(val, Ordering::Relaxed);
    }

    #[inline]
    pub fn packet_count(&self) -> u64 {
        self.packet_count.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn rx(&self) -> u64 {
        self.rx.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn tx(&self) -> u64 {
        self.tx.load(Ordering::Relaxed)
    }

    pub fn get(&self, channel: Option<Channel>) -> u64 {
        match channel {
            None => self.tx() + self.rx(),
            Some(Channel::Rx) => self.rx(),
            Some(Channel::Tx) => self.tx(),
        }
    }
}

impl Interest {
    pub fn interests_from_msg(msg: &SockMsgEvent) -> [Interest; 4] {
        [
            Interest::Pid(msg.pid),
            Interest::LocalSocket(msg.local_sock_addr()),
            Interest::RemoteSocket(msg.remote_sock_addr()),
            Interest::RemoteIp(msg.remote_addr.into()),
        ]
    }
}

#[derive(Copy, Clone, Eq, Debug)]
pub struct Socket {
    pub pid: u32,
    pub local: SocketAddr,
    pub remote: SocketAddr,
    pub sock_type: SockType,
}

impl From<&SockMsgEvent> for Socket {
    fn from(msg: &SockMsgEvent) -> Self {
        Self {
            pid: msg.pid,
            local: msg.local_sock_addr(),
            remote: msg.remote_sock_addr(),
            sock_type: msg.sock_type,
        }
    }
}

impl PartialEq for Socket {
    fn eq(&self, rhs: &Self) -> bool {
        self.local == rhs.local
    }
}

impl Hash for Socket {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.local.hash(state);
    }
}

#[derive(Debug, Default)]
pub struct Segment {
    total: Stat,
    index: DashMap<Interest, Stat, FxBuildHasher>,
    socks: DashSet<Socket, FxBuildHasher>,
}

impl Segment {
    pub fn batch_update<'a>(&self, messages: impl IntoIterator<Item = &'a SockMsgEvent>) {
        let mut rx: u64 = 0;
        let mut tx: u64 = 0;

        for msg in messages {
            if let Ok(len) = msg.packet_size() {
                let len = len.into();
                match msg.channel {
                    Channel::Tx => tx += len,
                    Channel::Rx => rx += len,
                }

                self.socks.insert(msg.into());

                for interest in Interest::interests_from_msg(msg) {
                    self.index
                        .entry(interest)
                        .and_modify(|stat| stat.add(msg.channel, len))
                        .or_insert_with(|| {
                            let stat = Stat::default();
                            stat.add(msg.channel, len);
                            stat
                        });
                }
            }
        }

        self.total.add(Channel::Rx, rx);
        self.total.add(Channel::Tx, tx);
    }

    #[inline]
    pub fn total(&self, channel: Option<Channel>) -> u64 {
        self.total.get(channel)
    }

    #[inline]
    pub fn total_packet_count(&self) -> u64 {
        self.total.packet_count()
    }

    pub fn by_interest(&self, interest: &Interest, channel: Option<Channel>) -> Option<u64> {
        self.index.get(interest).map(|stat| stat.get(channel))
    }

    pub fn for_each_sock(&self, mut f: impl FnMut(&Socket)) {
        self.socks.iter().for_each(|sock| f(sock.deref()));
    }
}

struct WriteTimeSegment<'a>(RwLockReadGuard<'a, VecDeque<TimeSegment>>);

impl<'a> Deref for WriteTimeSegment<'a> {
    type Target = TimeSegment;

    #[inline]
    fn deref(&self) -> &Self::Target {
        self.0.back().unwrap()
    }
}

/// A reader for a sequence of time segments.
///
/// This struct provides a read-only view of a sequence of `TimeSegment` objects in the storage of a `Store`.
///
/// Note that creating a `TimeSegmentsView` requires acquiring a read lock on the storage of the `Store`.
/// This means that while a `TimeSegmentsView` exists, new time segments cannot be added to the storage,
/// but existing time segments can still be updated.
pub struct TimeSegmentsView<'a>(RwLockReadGuard<'a, VecDeque<TimeSegment>>);

impl TimeSegmentsView<'_> {
    /// Returns the number of time segments in the reader.
    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if the reader contains no time segments.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns a reference to the first time segment in the reader, or `None` if the reader is empty.
    #[inline]
    pub fn first(&self) -> Option<&TimeSegment> {
        self.0.front()
    }

    /// Returns a reference to the last time segment in the reader, or `None` if the reader is empty.
    #[inline]
    pub fn last(&self) -> Option<&TimeSegment> {
        self.0.back()
    }

    /// Returns an iterator over the time segments in the reader.
    #[inline]
    pub fn iter(
        &self,
    ) -> impl Iterator<Item = &TimeSegment>
           + ExactSizeIterator<Item = &TimeSegment>
           + DoubleEndedIterator<Item = &TimeSegment>
           + '_ {
        self.0.iter()
    }
}

/// Store for time series data.
///
/// The Store struct maintains a list of TimeSegment instances, with each segment representing
/// a fixed time interval or window. The store aggregates the data within each segment and
/// provides an overall view of the data over the entire time period covered by the store.
pub struct Store {
    window: Duration,
    capacity: usize,
    segments: RwLock<VecDeque<TimeSegment>>,
}

impl Store {
    /// Constructor method that returns a new instance of the store.
    ///
    /// # Arguments
    ///
    /// * `window`: A Duration representing the time interval between each segment in the store.
    /// * `capacity`: An usize representing the maximum number of segments that the store can hold.
    pub fn new(window: Duration, capacity: usize) -> Self {
        let deque = VecDeque::with_capacity(capacity);

        Self {
            window,
            capacity,
            segments: RwLock::new(deque),
        }
    }

    /// Update the store from the messages.
    ///
    /// The `ts` parameter represents the timestamp of the update and must be from the same monolithic clock.
    pub fn batch_update<'a>(
        &self,
        ts: Timestamp,
        messages: impl IntoIterator<Item = &'a SockMsgEvent>,
    ) {
        let time_segment = self.write_segment(ts);
        time_segment.segment.batch_update(messages);
    }

    /// Returns a `TimeSegmentsView` that provides a read-only view of the time segments in the store.
    ///
    /// The `TimeSegmentsView` holds a read lock over the storage in the store,
    /// which prevents new time segments from being added while the reader is active.
    pub fn segments_view(&self) -> TimeSegmentsView<'_> {
        TimeSegmentsView(self.segments.read().unwrap())
    }

    fn write_segment(&self, ts: Timestamp) -> WriteTimeSegment<'_> {
        {
            // fast path: the current segment is not outdated.

            let read_guard = self.segments.read().unwrap();
            let dequeue = &*read_guard;

            if let Some(segment) = dequeue.back() {
                if segment.ts.saturating_elapsed_since(&ts) <= self.window {
                    return WriteTimeSegment(read_guard);
                }
            }
        };

        // FIXME(gwik): readers may prevent write access.
        match self.segments.try_write() {
            Ok(mut write_guard) => {
                let dequeue = &mut *write_guard;

                if let Some(segment) = dequeue.back() {
                    // Happends if an other thread creates the new segments
                    // after we released the read lock and before we grab the write
                    // lock.

                    if segment.ts.saturating_elapsed_since(&ts) <= self.window {
                        // Ignoring the race condition on purpose because it's harmless.
                        return WriteTimeSegment(self.segments.read().unwrap());
                    }
                }

                // We won the race to create the new segment.

                let segment = TimeSegment {
                    ts,
                    segment: Segment::default(),
                };

                if dequeue.len() >= self.capacity {
                    dequeue.pop_front();
                }
                dequeue.push_back(segment);
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Poisoned(e)) => panic!("poisoned lock {}", e),
        }

        // Ignoring the race condition on purpose because it's harmless.
        WriteTimeSegment(self.segments.read().unwrap())
    }
}

#[derive(Debug)]
pub struct TimeSegment {
    pub ts: Timestamp,
    pub segment: Segment,
}

#[cfg(test)]
mod tests {
    use crate::clock::ClockNano;
    use std::time::Duration;

    use super::*;

    #[test]
    fn store_batch_update_simple() {
        let window = Duration::from_millis(100);
        let store = Store::new(window, 1 << 4);
        let clock = ClockNano::default();
        let ts = clock.now();

        let messages = vec![
            SockMsgEvent {
                pid: 1,
                channel: Channel::Tx,
                sock_type: SockType::Stream,
                local_addr: ptraf_common::IpAddr::v4(33),
                local_port: 31,
                remote_addr: ptraf_common::IpAddr::v4(32),
                remote_port: 80,
                ret: 10,
            },
            SockMsgEvent {
                pid: 1,
                channel: Channel::Rx,
                sock_type: SockType::Stream,
                local_addr: ptraf_common::IpAddr::v4(33),
                local_port: 31,
                remote_addr: ptraf_common::IpAddr::v4(32),
                remote_port: 80,
                ret: 11,
            },
            SockMsgEvent {
                pid: 2,
                channel: Channel::Tx,
                sock_type: SockType::Stream,
                local_addr: ptraf_common::IpAddr::v4(33),
                local_port: 32,
                remote_addr: ptraf_common::IpAddr::v4(35),
                remote_port: 443,
                ret: 12,
            },
            SockMsgEvent {
                pid: 3,
                channel: Channel::Tx,
                sock_type: SockType::Stream,
                local_addr: ptraf_common::IpAddr::v4(33),
                local_port: 33,
                remote_addr: ptraf_common::IpAddr::v4(32),
                remote_port: 443,
                ret: 13,
            },
        ];

        store.batch_update(ts, &messages);
        store.batch_update(ts, &messages);
        store.batch_update(ts, &messages);
        store.batch_update(ts, &messages);

        let view = store.segments_view();
        assert_eq!(1, view.len());

        let time_segment = view.first().expect("first segment when not empty");
        assert_eq!(time_segment.ts, ts);

        let rx = time_segment.segment.total(Channel::Rx.into());
        let tx = time_segment.segment.total(Channel::Tx.into());

        assert_eq!(4 * (10 + 12 + 13), tx);
        assert_eq!(4 * 11, rx);

        assert_eq!(
            4 * (10 + 11 + 13),
            time_segment
                .segment
                .by_interest(
                    &Interest::RemoteIp(ptraf_common::IpAddr::v4(32).into()),
                    None,
                )
                .unwrap_or(0)
        );

        assert_eq!(
            4 * (10 + 11 + 13),
            time_segment
                .segment
                .by_interest(
                    &Interest::RemoteSocket(
                        (IpAddr::from(ptraf_common::IpAddr::v4(32)), 80).into()
                    ),
                    None,
                )
                .unwrap_or(0)
        );
    }
}
