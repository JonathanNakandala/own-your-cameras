//! Shared transport with the platform runtime progress adapter.
pub(crate) use camera_transport::rtsp::proto;
pub use camera_transport::rtsp::{audio, hub, talk};
pub mod server;

/// The Opus codec for both audio directions.
pub use camera_opus as opus;
