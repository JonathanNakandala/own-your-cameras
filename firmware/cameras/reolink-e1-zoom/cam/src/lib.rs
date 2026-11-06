//! Firmware user space for the Reolink E1 Zoom: one process that owns the camera's hardware and serves video,
//! two-way audio and pan/tilt/zoom to the home network.

// What runs: the subcommands, and the production runtime the launcher starts.
pub mod cmd;
pub mod exclusion;
pub mod identity;
pub mod ownership;
pub mod preflight;
pub mod runtime;

// What it serves.
mod media_tuning;
pub use camera_control::motion_control;
pub mod mqtt;
pub mod net;
pub mod pipeline;
pub mod position3d;
pub mod presets;
pub mod rtsp;
pub mod settings;
pub mod telemetry;

// The hardware it drives.
pub mod control_stop;
pub mod ir;
mod isp_iq;
pub mod orientation;
pub mod ptz;
pub mod wdt;
pub mod zoom;

// Plumbing.
mod annexb;
mod cpu;
#[cfg(target_arch = "arm")]
mod eac;
pub mod files;
mod hardware_fd;
pub mod logcap;
pub mod selftest;
pub mod signals;
