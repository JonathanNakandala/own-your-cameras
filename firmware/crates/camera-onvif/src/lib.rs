//! ONVIF Profile S (Media1) for the cameras of this workspace: WS-Discovery, the Device, Media, PTZ
//! and Events services, and an authenticated hardware-JPEG snapshot URL.
//!
//! Layers: a pure domain (`xml`, `codec`, `time`, `soap`, `auth`, `discovery`, `service`, `events`,
//! `snapshot`), the [`OnvifCamera`] port each camera implements, thin adapters that do the I/O
//! (`http`, `udp`, `netif`), and the shared wiring in `runtime`.

pub mod auth;
pub mod camera;
pub mod codec;
pub mod discovery;
pub mod entropy;
pub mod events;
#[cfg(test)]
mod fake;
pub mod http;
pub mod netif;
pub mod runtime;
pub mod service;
pub mod snapshot;
pub mod soap;
pub mod time;
pub mod udp;
pub mod xml;

pub use camera::{
    Absolute, DeviceIdentity, H264Profile, ImagingError, ImagingOptions, ImagingPort,
    ImagingSettings, IrCutFilter, MediaProfile, OnvifCamera, Pose, Preset, PtzError, PtzFeatures,
    PtzPort, Relative, RelayError, SnapshotError, Velocity, VideoEncoding,
};
pub use http::{Events, HttpLimits};
pub use runtime::{Product, Service, identity, start};
