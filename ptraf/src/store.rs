use std::{
    collections::VecDeque,
    hash::Hash,
    net::{IpAddr, SocketAddr},
    ops::{AddAssign, Deref},
    sync::{
        atomic::{AtomicU64, Ordering},
        RwLock, RwLockReadGuard,
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
struct Traffic {
    size: AtomicU64,
    count: AtomicU64,
}

impl Traffic {
    #[inline]
    fn increment(&self, val: u64, count: u64) {
        self.size.fetch_add(val, Ordering::Relaxed);
        self.count.fetch_add(count, Ordering::Relaxed);
    }

    #[inline]
    fn size(&self) -> u64 {
        self.size.load(Ordering::Relaxed)
    }

    #[inline]
    fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Stat {
    pub rx: u64,
    pub rx_packet_count: u64,
    pub tx: u64,
    pub tx_packet_count: u64,
}

impl Stat {
    pub fn total(&self) -> u64 {
        self.rx + self.tx
    }

    pub fn merge(&mut self, other: &Self) {
        *self = Self {
            rx: self.rx + other.rx,
            rx_packet_count: self.rx_packet_count + other.rx_packet_count,
            tx: self.tx + other.tx,
            tx_packet_count: self.tx_packet_count + other.tx_packet_count,
        }
    }
}

impl AddAssign<Stat> for Stat {
    fn add_assign(&mut self, rhs: Stat) {
        self.merge(&rhs)
    }
}

impl AddAssign<&'_ Stat> for Stat {
    fn add_assign(&mut self, rhs: &Stat) {
        self.merge(rhs)
    }
}

impl From<&'_ Metrics> for Stat {
    fn from(m: &Metrics) -> Self {
        Self {
            rx: m.rx.size(),
            rx_packet_count: m.rx.count(),
            tx: m.tx.size(),
            tx_packet_count: m.tx.count(),
        }
    }
}

#[derive(Debug, Default)]
struct Metrics {
    rx: Traffic,
    tx: Traffic,
}

impl Metrics {
    #[inline]
    fn with_channel(&self, channel: Channel) -> &Traffic {
        match channel {
            Channel::Rx => &self.rx,
            Channel::Tx => &self.tx,
        }
    }

    #[inline]
    fn increment(&self, channel: Channel, val: u64, count: u64) {
        self.with_channel(channel).increment(val, count)
    }

    #[inline]
    pub fn packet_count(&self) -> u64 {
        self.tx.count() + self.rx.count()
    }

    #[inline]
    pub fn rx(&self) -> u64 {
        self.rx.size()
    }

    #[inline]
    pub fn tx(&self) -> u64 {
        self.tx.size()
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
    total: Metrics,
    index: DashMap<Interest, Metrics, FxBuildHasher>,
    socks: DashSet<Socket, FxBuildHasher>,
}

impl Segment {
    pub fn batch_update<'a>(&self, messages: impl IntoIterator<Item = &'a SockMsgEvent>) {
        let mut rx: u64 = 0;
        let mut tx: u64 = 0;

        let mut rx_n: u64 = 0;
        let mut tx_n: u64 = 0;

        for msg in messages {
            if let Ok(len) = msg.packet_size() {
                let len = len.into();
                match msg.channel {
                    Channel::Tx => {
                        tx += len;
                        tx_n += 1;
                    }
                    Channel::Rx => {
                        rx += len;
                        rx_n += 1;
                    }
                }

                self.socks.insert(msg.into());

                for interest in Interest::interests_from_msg(msg) {
                    self.index
                        .entry(interest)
                        .and_modify(|stat| {
                            stat.increment(msg.channel, len, 1);
                        })
                        .or_insert_with(|| {
                            let stat = Metrics::default();
                            stat.increment(msg.channel, len, 1);
                            stat
                        });
                }
            }
        }

        self.total.increment(Channel::Rx, rx, rx_n);
        self.total.increment(Channel::Tx, tx, tx_n);
    }

    #[inline]
    pub fn total(&self, channel: Option<Channel>) -> u64 {
        self.total.get(channel)
    }

    #[inline]
    pub fn total_packet_count(&self) -> u64 {
        self.total.packet_count()
    }

    pub fn stat_by_interest(&self, interest: &Interest) -> Option<Stat> {
        self.index.get(interest).map(|m| (&*m).into())
    }

    pub fn socket_iter(&self) -> impl Iterator<Item = Socket> + '_ {
        self.socks.iter().map(|sock| *sock)
    }

    pub fn for_each_socket(&self, mut f: impl FnMut(&Socket)) {
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
#[derive(Debug)]
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

    pub fn window(&self) -> Duration {
        self.window
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
        let ts = ts.trunc(self.window);

        {
            // fast path: the current segment is not outdated.

            let read_guard = self.segments.read().unwrap();
            let dequeue = &*read_guard;

            if let Some(segment) = dequeue.back() {
                if segment.ts == ts {
                    return WriteTimeSegment(read_guard);
                }
            }
        };

        let mut write_guard = self.segments.write().unwrap();
        let dequeue = &mut *write_guard;

        loop {
            // slow path: create missing segments.

            let segment_ts = if let Some(segment) = dequeue.back() {
                // Happends if an other thread creates the new segments
                // after we released the read lock and before we grab the write
                // lock.

                if segment.ts >= ts {
                    // Ignoring the race condition on purpose because it's harmless.
                    drop(write_guard);
                    return WriteTimeSegment(self.segments.read().unwrap());
                }
                segment.ts + self.window
            } else {
                ts
            };

            if dequeue.len() >= self.capacity {
                dequeue.pop_front();
            }
            dequeue.push_back(TimeSegment {
                ts: segment_ts,
                segment: Segment::default(),
            });
        }
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
        let store = Store::new(window, 16);
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
        assert_eq!(time_segment.ts, ts.trunc(window));

        let rx = time_segment.segment.total(Channel::Rx.into());
        let tx = time_segment.segment.total(Channel::Tx.into());

        assert_eq!(4 * (10 + 12 + 13), tx);
        assert_eq!(4 * 11, rx);

        assert_eq!(
            4 * (10 + 11 + 13),
            time_segment
                .segment
                .stat_by_interest(&Interest::RemoteIp(ptraf_common::IpAddr::v4(32).into()),)
                .map(|stat| stat.total())
                .unwrap_or(0)
        );

        assert_eq!(
            4 * (10 + 11),
            time_segment
                .segment
                .stat_by_interest(&Interest::RemoteSocket(
                    (IpAddr::from(ptraf_common::IpAddr::v4(32)), 80).into()
                ))
                .map(|stat| stat.total())
                .unwrap_or(0)
        );
    }

    #[test]
    fn store_create_segments() {
        let messages = vec![SockMsgEvent {
            pid: 1,
            channel: Channel::Tx,
            sock_type: SockType::Stream,
            local_addr: ptraf_common::IpAddr::v4(33),
            local_port: 31,
            remote_addr: ptraf_common::IpAddr::v4(32),
            remote_port: 80,
            ret: 10,
        }];

        let window = Duration::from_millis(100);
        let store = Store::new(window, 16);

        store.batch_update(Duration::from_millis(10).into(), &messages); // 0
        store.batch_update(Duration::from_millis(20).into(), &messages); // 0
        store.batch_update(Duration::from_millis(100).into(), &messages); // 1
        store.batch_update(Duration::from_millis(101).into(), &messages); // 1
        store.batch_update(Duration::from_millis(401).into(), &messages); // 4

        let view = store.segments_view();

        let times: Vec<_> = view
            .iter()
            .map(|TimeSegment { ts, segment }| (*ts, segment.total_packet_count()))
            .collect();

        assert_eq!(
            times,
            vec![
                (Duration::ZERO.into(), 2),
                (Duration::from_millis(100).into(), 2),
                (Duration::from_millis(200).into(), 0),
                (Duration::from_millis(300).into(), 0),
                (Duration::from_millis(400).into(), 1),
            ]
        );
    }
}
