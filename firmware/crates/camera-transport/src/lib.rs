//! Hardware-independent camera transport, shared by the ARM and MIPS applications.
pub mod annexb;
pub mod microphone;
pub mod mqtt;
pub mod opus;
pub mod resample;
pub mod rtsp;
pub mod settings;
pub mod sntp;
pub mod telemetry;
pub use camera_control::{motion_control, position3d};
pub use rtsp::audio::AudioHub;
pub use rtsp::hub::{Hub, VideoHubs, VideoLimits};
pub use rtsp::proto::Credentials;
pub use rtsp::server::{
    Backchannel, Config, MAIN_PATH, MAIN_PCMU_PATH, SUB_PATH, SUB_PCMU_PATH, ServerLimits,
    StreamPath, bind, bind_with_limits, serve, serve_with, serve_with_limits,
};
pub use rtsp::talk::Talkback;
