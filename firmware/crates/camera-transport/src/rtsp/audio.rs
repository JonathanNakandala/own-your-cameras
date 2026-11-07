//! Microphone fan-out at the RTSP boundary: one encoded packet stream shared by every listener.

use crate::microphone::Format;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, Weak};

pub struct AudioFrame {
    pub pts_us: u64,
    pub data: Vec<u8>,
}

/// One microphone capture shared by both RTSP paths; at most 16 blocks (320 ms of Opus) queued per viewer.
#[derive(Default)]
pub struct AudioHub {
    format: Format,
    subs: Mutex<Vec<AudioSub>>,
}

struct AudioSub {
    tx: SyncSender<Arc<AudioFrame>>,
    viewer: Weak<()>,
}

pub struct AudioViewer {
    rx: Receiver<Arc<AudioFrame>>,
    _alive: Arc<()>,
}

impl AudioViewer {
    pub fn next(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Arc<AudioFrame>, std::sync::mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

impl AudioHub {
    /// An Opus 48 kHz microphone.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
    pub fn with_format(format: Format) -> Arc<Self> {
        Arc::new(Self {
            format,
            subs: Mutex::default(),
        })
    }
    pub fn format(&self) -> Format {
        self.format
    }
    /// Current microphone viewers, including ones waiting for their first frame.
    pub fn viewers(&self) -> usize {
        self.subs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|s| s.viewer.strong_count() != 0)
            .count()
    }
    pub fn subscribe(&self) -> AudioViewer {
        let (tx, rx) = sync_channel(16);
        let alive = Arc::new(());
        let mut subs = self.subs.lock().unwrap_or_else(|e| e.into_inner());
        subs.retain(|s| s.viewer.strong_count() != 0);
        subs.push(AudioSub {
            tx,
            viewer: Arc::downgrade(&alive),
        });
        AudioViewer { rx, _alive: alive }
    }
    pub fn publish(&self, pts_us: u64, data: Vec<u8>) {
        if data.is_empty() || data.len() > 8191 {
            return;
        }
        let data = data.into_boxed_slice().into_vec();
        let frame = Arc::new(AudioFrame { pts_us, data });
        self.subs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|s| {
                s.viewer.strong_count() != 0
                    && !matches!(
                        s.tx.try_send(Arc::clone(&frame)),
                        Err(TrySendError::Disconnected(_))
                    )
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn audio_fanout_drops_lag_without_blocking_other_listeners() {
        let hub = AudioHub::new();
        let slow = hub.subscribe();
        let fast = hub.subscribe();
        for pts in 0..100 {
            hub.publish(pts, vec![7]);
            assert_eq!(fast.next(Duration::ZERO).unwrap().pts_us, pts);
        }
        let mut count = 0;
        while slow.next(Duration::ZERO).is_ok() {
            count += 1;
        }
        assert_eq!(count, 16);
    }
}
