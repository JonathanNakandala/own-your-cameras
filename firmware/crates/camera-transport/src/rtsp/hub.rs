//! Fan-out of one encoder stream to its RTSP viewers. A frame is allocated once by the encoder backend and
//! shared as an `Arc` with every viewer; a viewer that cannot keep up skips to the next key frame instead
//! of making the encoder loop (or the other viewers) wait.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use crate::annexb::nal_types;

use super::audio::AudioHub;
use super::proto::VideoSdp;
use super::server::{Backchannel, MAIN_PATH, MAIN_PCMU_PATH, SUB_PATH, SUB_PCMU_PATH, StreamPath};

#[derive(Debug)]
pub struct Frame {
    pub key: bool,
    pub pts_us: u64,
    /// Annex B access unit.
    pub data: Vec<u8>,
    _charge: Charge,
}

/// Shared by both production streams, counting queued and currently transmitted allocations once.
pub const RETAINED_BYTES: usize = 8 * 1024 * 1024;
const PARAM_BYTES: usize = 4096;
/// Admission limits are per hub, never workspace-wide Cargo features.
#[derive(Clone, Copy, Debug)]
pub struct VideoLimits {
    max_au_bytes: usize,
    retained_bytes: usize,
    queue: usize,
    max_nals: usize,
}
impl VideoLimits {
    pub const fn max_au_bytes(self) -> usize {
        self.max_au_bytes
    }
    pub const fn max_nals(self) -> usize {
        self.max_nals
    }
    pub const REOLINK: Self = Self {
        max_au_bytes: 7 * 1024 * 1024,
        retained_bytes: RETAINED_BYTES,
        queue: 32,
        max_nals: 256,
    };
    pub const MI360: Self = Self {
        max_au_bytes: 1024 * 1024,
        retained_bytes: 2 * 1024 * 1024,
        queue: 4,
        max_nals: 256,
    };
}
impl Default for VideoLimits {
    fn default() -> Self {
        Self::REOLINK
    }
}
#[cfg(test)]
const QUEUE: usize = VideoLimits::REOLINK.queue;

#[derive(Debug)]
struct Charge {
    used: Arc<AtomicUsize>,
    bytes: usize,
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

struct Sub {
    tx: SyncSender<Arc<Frame>>,
    need_key: bool,
    viewer: Weak<()>,
}

#[derive(Default)]
struct Inner {
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
    subs: Vec<Sub>,
}

#[derive(Default)]
pub struct Hub {
    limits: VideoLimits,
    inner: Mutex<Inner>,
    params_ready: Condvar,
    retained: Arc<AtomicUsize>,
}

/// The hub dropped its end: no more frames will come.
#[derive(Debug, PartialEq, Eq)]
pub struct Closed;

/// A viewer's end of a [`Hub`].
pub struct Viewer {
    rx: Receiver<Arc<Frame>>,
    _alive: Arc<()>,
}

impl Viewer {
    /// Waits up to `d` for the next frame. `None` on timeout; `Err` once the hub is gone.
    pub fn next(&self, d: Duration) -> Result<Option<Arc<Frame>>, Closed> {
        match self.rx.recv_timeout(d) {
            Ok(f) => Ok(Some(f)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(_) => Err(Closed),
        }
    }

    pub fn try_next(&self) -> Result<Option<Arc<Frame>>, Closed> {
        match self.rx.try_recv() {
            Ok(f) => Ok(Some(f)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(Closed),
        }
    }
}

fn nal_of(data: &[u8], ty: u8) -> Option<Vec<u8>> {
    let nals = crate::annexb::nal_slices(data);
    let types = nal_types(data);
    types
        .iter()
        .position(|t| *t == ty)
        .map(|i| nals[i].to_vec())
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Publishes one access unit. SPS/PPS are remembered; a key frame that arrives without them gets them
    /// prepended (the only case where the data is copied).
    // `fetch_update` is named `try_update` from Rust 1.99, which the supported 1.88 does not have.
    #[allow(deprecated)]
    pub fn publish(&self, key: bool, pts_us: u64, mut data: Vec<u8>) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Bound parsing metadata and caches before any NAL-sized allocation.
        if data.len() > self.limits.max_au_bytes
            || data.windows(3).filter(|w| *w == [0, 0, 1]).count() > self.limits.max_nals
        {
            for s in &mut g.subs {
                s.need_key = true;
            }
            return;
        }
        let types = nal_types(&data);
        let nals = crate::annexb::nal_slices(&data);
        if nals
            .iter()
            .any(|n| matches!(n[0] & 31, 7 | 8) && n.len() > PARAM_BYTES)
        {
            for s in &mut g.subs {
                s.need_key = true;
            }
            return;
        }
        if types.contains(&7)
            && let Some(s) = nal_of(&data, 7)
        {
            g.sps = Some(s);
        }
        if types.contains(&8)
            && let Some(p) = nal_of(&data, 8)
        {
            g.pps = Some(p);
        }
        if key
            && !(types.contains(&7) && types.contains(&8))
            && let (Some(s), Some(p)) = (g.sps.clone(), g.pps.clone())
        {
            let size = data.len() + s.len() + p.len() + 8;
            if size > self.limits.max_au_bytes {
                for s in &mut g.subs {
                    s.need_key = true;
                }
                return;
            }
            let mut with = Vec::with_capacity(size);
            for nal in [&s, &p] {
                with.extend_from_slice(&[0, 0, 0, 1]);
                with.extend_from_slice(nal);
            }
            with.extend_from_slice(&data);
            data = with;
        }
        if g.sps.is_some() && g.pps.is_some() {
            self.params_ready.notify_all();
        }
        // Strip spare Vec capacity so the charge covers the actual retained allocation.
        let data = data.into_boxed_slice().into_vec();
        let bytes = data.len() + std::mem::size_of::<Frame>();
        if self
            .retained
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|n| *n <= self.limits.retained_bytes)
            })
            .is_err()
        {
            for s in &mut g.subs {
                s.need_key = true;
            }
            return;
        }
        let frame = Arc::new(Frame {
            key,
            pts_us,
            data,
            _charge: Charge {
                used: Arc::clone(&self.retained),
                bytes,
            },
        });
        g.subs.retain_mut(|s| {
            if s.viewer.strong_count() == 0 {
                return false;
            }
            if s.need_key && !key {
                return true;
            }
            match s.tx.try_send(Arc::clone(&frame)) {
                Ok(()) => {
                    s.need_key = false;
                    true
                }
                Err(TrySendError::Full(_)) => {
                    s.need_key = true;
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }

    /// SPS and PPS of the stream, once the first key frame went through.
    pub fn params(&self) -> Option<VideoSdp> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        Some(VideoSdp {
            sps: g.sps.clone()?,
            pps: g.pps.clone()?,
        })
    }

    /// Like [`Hub::params`], waiting up to `d` for the encoder to produce its first key frame.
    pub fn wait_params(&self, d: Duration) -> Option<VideoSdp> {
        let g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let (g, _) = self
            .params_ready
            .wait_timeout_while(g, d, |i| i.sps.is_none() || i.pps.is_none())
            .unwrap_or_else(|e| e.into_inner());
        Some(VideoSdp {
            sps: g.sps.clone()?,
            pps: g.pps.clone()?,
        })
    }

    /// A viewer that starts at the next key frame.
    pub fn subscribe(&self) -> Viewer {
        let (tx, rx) = sync_channel(self.limits.queue);
        let alive = Arc::new(());
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.subs.retain(|s| s.viewer.strong_count() != 0);
        g.subs.push(Sub {
            tx,
            need_key: true,
            viewer: Arc::downgrade(&alive),
        });
        Viewer { rx, _alive: alive }
    }

    /// Allocations retained by the shared stream budget, including active senders.
    pub fn retained_bytes(&self) -> usize {
        self.retained.load(Ordering::Acquire)
    }

    pub fn viewers(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .subs
            .iter()
            .filter(|s| s.viewer.strong_count() != 0)
            .count()
    }
}

/// The camera's two video streams, so callers say "main"/"sub" instead of indexing: pipeline stream `0` is
/// main, `1` is sub (`pipeline::PipelineConfig::streams` order).
pub struct VideoHubs {
    pub main: Arc<Hub>,
    pub sub: Arc<Hub>,
}

impl VideoHubs {
    pub fn new() -> Self {
        Self::with_limits(VideoLimits::REOLINK)
    }

    pub fn with_limits(limits: VideoLimits) -> Self {
        let retained = Arc::new(AtomicUsize::new(0));
        Self {
            main: Arc::new(Hub {
                limits,
                retained: Arc::clone(&retained),
                ..Hub::default()
            }),
            sub: Arc::new(Hub {
                limits,
                retained,
                ..Hub::default()
            }),
        }
    }

    /// Classify a bounded hardware access unit without allocating unbounded NAL metadata.
    pub fn publish_annexb(&self, stream: usize, pts_us: u64, data: Vec<u8>) {
        let hub = if stream == 0 { &self.main } else { &self.sub };
        let limits = hub.limits;
        let bounded = data.len() <= limits.max_au_bytes
            && data
                .windows(3)
                .filter(|w| *w == [0, 0, 1])
                .take(limits.max_nals + 1)
                .count()
                <= limits.max_nals;
        let key = bounded && crate::annexb::nal_types(&data).contains(&5);
        hub.publish(key, pts_us, data);
    }

    /// Every RTSP path of a camera with these hubs: its own paths with the Opus microphone and an Opus
    /// backchannel offer, then the ONVIF paths with the PCMU microphone and a PCMU offer.
    pub fn paths(
        &self,
        opus: Option<Arc<AudioHub>>,
        pcmu: Option<Arc<AudioHub>>,
    ) -> Vec<StreamPath> {
        let own = [(MAIN_PATH, &self.main), (SUB_PATH, &self.sub)]
            .map(|(path, hub)| StreamPath::new(path, Arc::clone(hub)).audio(opus.clone()));
        let onvif =
            [(MAIN_PCMU_PATH, &self.main), (SUB_PCMU_PATH, &self.sub)].map(|(path, hub)| {
                StreamPath::new(path, Arc::clone(hub))
                    .audio(pcmu.clone())
                    .backchannel(Backchannel::Pcmu)
            });
        own.into_iter().chain(onvif).collect()
    }

    /// Publishes an encoder frame to the hub of pipeline stream `stream` (0 main, otherwise sub).
    pub fn publish(&self, stream: usize, key: bool, pts_us: u64, data: Vec<u8>) {
        let hub = if stream == 0 { &self.main } else { &self.sub };
        hub.publish(key, pts_us, data);
    }
}

impl Default for VideoHubs {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &[u8] = &[0, 0, 0, 1, 0x67, 0x4d, 0, 0x1f];
    const PPS: &[u8] = &[0, 0, 0, 1, 0x68, 0xee];
    const T: Duration = Duration::from_millis(10);

    fn idr() -> Vec<u8> {
        [SPS, PPS, &[0, 0, 0, 1, 0x65, 1, 2, 3]].concat()
    }

    #[test]
    fn simultaneous_camera_profiles_have_independent_limits_and_shared_stream_budgets() {
        let small = VideoHubs::with_limits(VideoLimits::MI360);
        let large = VideoHubs::with_limits(VideoLimits::REOLINK);
        let a = small.main.subscribe();
        let b = small.sub.subscribe();
        let c = large.main.subscribe();
        let make = |bytes| {
            let mut data = vec![1; bytes];
            data[..5].copy_from_slice(&[0, 0, 0, 1, 0x65]);
            data
        };
        small.publish_annexb(0, 1, make(2 * 1024 * 1024));
        large.publish_annexb(0, 1, make(2 * 1024 * 1024));
        assert!(
            a.try_next().unwrap().is_none(),
            "Mi360 refuses an oversized access unit"
        );
        let inflight = c.try_next().unwrap().unwrap();
        assert!(large.main.retained_bytes() > 2 * 1024 * 1024);
        for (stream, pts) in [(0, 2), (1, 3), (0, 4)] {
            small.publish_annexb(stream, pts, make(768 * 1024));
        }
        assert_eq!(a.try_next().unwrap().unwrap().pts_us, 2);
        assert!(
            a.try_next().unwrap().is_none(),
            "main/sub share Mi360's two MiB budget"
        );
        assert_eq!(b.try_next().unwrap().unwrap().pts_us, 3);
        assert_eq!(small.main.retained_bytes(), 0);
        assert!(
            large.main.retained_bytes() > 0,
            "one camera's drain cannot release another camera's allocations"
        );
        drop(inflight);
        assert_eq!(large.main.retained_bytes(), 0);
        assert_eq!(small.main.limits.queue, 4);
        assert_eq!(large.main.limits.queue, 32);
    }

    #[test]
    fn viewer_starts_at_a_key_frame_and_shares_the_allocation() {
        let hub = Hub::new();
        let v = hub.subscribe();
        hub.publish(false, 0, vec![0, 0, 0, 1, 0x41, 9]);
        assert!(v.next(T).unwrap().is_none());
        hub.publish(true, 1, idr());
        let w = hub.subscribe();
        hub.publish(true, 2, idr());
        let a = v.next(T).unwrap().unwrap();
        assert_eq!(a.pts_us, 1);
        let b = v.next(T).unwrap().unwrap();
        let c = w.next(T).unwrap().unwrap();
        assert!(Arc::ptr_eq(&b, &c));
    }

    #[test]
    fn remembers_params_and_adds_them_to_a_key_frame_without() {
        let hub = Hub::new();
        assert!(hub.params().is_none());
        hub.publish(true, 0, idr());
        let p = hub.params().unwrap();
        assert_eq!(p.sps, SPS[4..]);
        assert_eq!(p.pps, PPS[4..]);
        let v = hub.subscribe();
        hub.publish(true, 1, vec![0, 0, 0, 1, 0x65, 7]);
        let f = v.next(T).unwrap().unwrap();
        assert_eq!(nal_types(&f.data), [7, 8, 5]);
    }

    #[test]
    fn wait_params_times_out_then_succeeds() {
        let hub = Hub::new();
        assert!(hub.wait_params(Duration::from_millis(5)).is_none());
        hub.publish(true, 0, idr());
        assert!(hub.wait_params(Duration::from_millis(5)).is_some());
    }

    #[test]
    fn slow_viewer_skips_to_the_next_key_frame_and_does_not_block() {
        let hub = Hub::new();
        let v = hub.subscribe();
        hub.publish(true, 0, idr());
        for i in 1..=QUEUE as u64 + 5 {
            hub.publish(false, i, vec![0, 0, 0, 1, 0x41, 1]);
        }
        hub.publish(true, 100, idr());
        let mut pts = Vec::new();
        while let Some(f) = v.try_next().unwrap() {
            pts.push(f.pts_us);
        }
        assert_eq!(pts.len(), QUEUE);
        assert_eq!(pts[0], 0);
        hub.publish(false, 101, vec![0, 0, 0, 1, 0x41, 1]);
        assert!(v.try_next().unwrap().is_none(), "waits for a key frame");
        hub.publish(true, 102, idr());
        assert_eq!(v.try_next().unwrap().unwrap().pts_us, 102);
    }

    #[test]
    fn both_streams_and_inflight_frames_share_the_byte_budget() {
        let hubs = VideoHubs::new();
        let a = hubs.main.subscribe();
        let b = hubs.sub.subscribe();
        let au = || {
            let mut v = vec![1; 2 * 1024 * 1024];
            v[..5].copy_from_slice(&[0, 0, 0, 1, 0x65]);
            v
        };
        hubs.main.publish(true, 1, au());
        let inflight = a.try_next().unwrap().unwrap();
        hubs.sub.publish(true, 2, au());
        hubs.main.publish(true, 3, au());
        hubs.sub.publish(true, 4, au()); // would exceed the global 8 MiB, even with an empty main queue
        assert_eq!(b.try_next().unwrap().unwrap().pts_us, 2);
        assert!(b.try_next().unwrap().is_none());
        assert!(hubs.main.retained.load(Ordering::Acquire) <= RETAINED_BYTES);
        drop(inflight);
        hubs.sub.publish(true, 5, au());
        assert_eq!(b.try_next().unwrap().unwrap().pts_us, 5);
        drop(a);
        drop(b);
        assert_eq!(hubs.main.retained.load(Ordering::Acquire), 0);
    }

    #[test]
    fn oversized_parameter_sets_never_replace_good_sdp() {
        let hub = Hub::new();
        hub.publish(true, 0, idr());
        let before = hub.params().unwrap().sps;
        let mut bad = vec![1; PARAM_BYTES + 5];
        bad[..5].copy_from_slice(&[0, 0, 0, 1, 0x67]);
        hub.publish(true, 1, bad);
        assert_eq!(hub.params().unwrap().sps, before);
    }

    #[test]
    fn dropped_viewer_is_removed() {
        let hub = Hub::new();
        let v = hub.subscribe();
        assert_eq!(hub.viewers(), 1);
        drop(v);
        hub.publish(true, 0, idr());
        assert_eq!(hub.viewers(), 0);
    }

    #[test]
    fn video_hubs_route_streams_to_their_paths() {
        let hubs = VideoHubs::new();
        let (main, sub) = (hubs.main.subscribe(), hubs.sub.subscribe());
        hubs.publish(1, true, 7, idr());
        assert!(main.try_next().unwrap().is_none());
        assert_eq!(sub.try_next().unwrap().unwrap().pts_us, 7);
        hubs.publish(0, true, 9, idr());
        assert_eq!(main.try_next().unwrap().unwrap().pts_us, 9);
        let paths = hubs.paths(None, None);
        let names: Vec<_> = paths.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(names, ["main", "sub", "main_pcmu", "sub_pcmu"]);
        assert!(
            Arc::ptr_eq(&paths[2].video, &hubs.main),
            "ONVIF paths share the camera's video"
        );
        assert_eq!(paths[3].backchannel, crate::rtsp::server::Backchannel::Pcmu);
    }
}
