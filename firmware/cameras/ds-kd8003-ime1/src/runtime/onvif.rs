//! The door panel side of ONVIF: profiles from the encoders that run, the call button and the door's reed
//! as digital inputs, and snapshots from the JPEG encoder channel. Everything else is `camera_onvif`'s,
//! shared with the other cameras.

use super::Device;
use super::ffi::gm_jpeg_shot;
use camera_onvif::{
    DeviceIdentity, H264Profile, MediaProfile, OnvifCamera, PtzPort, SnapshotError, VideoEncoding,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Digital input tokens: the call button and the door (reed open).
pub const CALL_BUTTON: &str = "call_button";
pub const DOOR: &str = "door";
const INPUTS: [&str; 2] = [CALL_BUTTON, DOOR];
/// Largest JPEG accepted (the encoder's buffer is half a 1080p picture).
const JPEG_BYTES: usize = 1024 * 1024;

pub struct DoorOnvif {
    pub identity: DeviceIdentity,
    pub device: Arc<Device>,
    /// Whether the JPEG channel came up; the buffer's lock serializes snapshots.
    pub jpeg: bool,
    pub buffer: Mutex<Vec<u8>>,
}

/// What `gm_stream_get` reports: w, h, fps, kbps, gop, mode, profile (0 baseline, 1 main, 2 high).
fn encoding(v: [u32; 7]) -> VideoEncoding {
    VideoEncoding {
        width: v[0],
        height: v[1],
        fps: v[2],
        bitrate_kbps: v[3],
        gop: v[4],
        profile: match v[6] {
            0 => H264Profile::Baseline,
            2 => H264Profile::High,
            _ => H264Profile::Main,
        },
    }
}

impl OnvifCamera for DoorOnvif {
    fn identity(&self) -> &DeviceIdentity {
        &self.identity
    }

    fn profiles(&self) -> Vec<MediaProfile> {
        let streams = self.device.st.lock().unwrap().streams;
        [
            ("main", camera_transport::MAIN_PCMU_PATH, streams[0]),
            ("sub", camera_transport::SUB_PCMU_PATH, streams[1]),
        ]
        .into_iter()
        .filter_map(|(token, stream_path, v)| {
            v.map(|v| MediaProfile {
                token,
                stream_path,
                video: encoding(v),
                audio: true,
            })
        })
        .collect()
    }

    fn audio_output(&self) -> bool {
        true
    }

    fn ptz(&self) -> Option<&dyn PtzPort> {
        None
    }

    fn digital_inputs(&self) -> &[&'static str] {
        &INPUTS
    }

    fn snapshot(&self, deadline: Instant) -> Result<Vec<u8>, SnapshotError> {
        if !self.jpeg {
            return Err(SnapshotError::Failed("no JPEG encoder".into()));
        }
        let mut buffer = self.buffer.lock().unwrap_or_else(|e| e.into_inner());
        buffer.resize(JPEG_BYTES, 0);
        let wait_ms = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(1500) as u32;
        let mut len = 0u32;
        // SAFETY: `buffer` holds JPEG_BYTES writable bytes for the call; the lock serializes channel 2.
        let rc = unsafe { gm_jpeg_shot(buffer.as_mut_ptr(), JPEG_BYTES as u32, &mut len, wait_ms) };
        match rc {
            0 if (len as usize) <= buffer.len() && buffer.starts_with(&[0xff, 0xd8]) => {
                Ok(buffer[..len as usize].to_vec())
            }
            0 => Err(SnapshotError::Failed("the encoder returned no JPEG".into())),
            -4 => Err(SnapshotError::NotReady),
            rc => Err(SnapshotError::Failed(format!("JPEG encoder: {rc}"))),
        }
    }
}
