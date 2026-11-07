//! Shared camera motion contract. Clients replace the complete desired vector;
//! axis combinations are values, never a growing list of arrow commands.
//! Hardware, clocks, MQTT and optical calibration stay behind device adapters.

use serde::Deserialize;

pub const MAX_PAYLOAD: usize = 256;
pub const VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresetName {
    bytes: [u8; 64],
    length: u8,
}
impl PresetName {
    pub fn parse(name: &str) -> Result<Self, String> {
        if name.is_empty()
            || name.len() > 64
            || name.trim() != name
            || name.chars().any(char::is_control)
        {
            return Err("invalid preset name".into());
        }
        let mut bytes = [0; 64];
        bytes[..name.len()].copy_from_slice(name.as_bytes());
        Ok(Self {
            bytes,
            length: name.len() as u8,
        })
    }
    pub fn as_str(&self) -> Result<&str, String> {
        std::str::from_utf8(&self.bytes[..usize::from(self.length)])
            .map_err(|_| "invalid preset name".into())
    }
}

/// Signed percentages of each axis's configured speed ceiling. Positive pan is
/// screen right, tilt is screen up, zoom is in. Missing axes mean zero.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Velocity {
    pub pan: i16,
    pub tilt: i16,
    pub zoom: i16,
}
impl Velocity {
    pub fn is_zero(self) -> bool {
        self == Self::default()
    }
    pub fn validate(self) -> Result<Self, String> {
        if [self.pan, self.tilt, self.zoom]
            .iter()
            .any(|v| !(-100..=100).contains(v))
        {
            return Err("motion velocity must be -100..100".into());
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Velocity(Velocity),
    Home,
    Preset(PresetName),
    /// Saves the current position under this name, creating the preset if the store has room.
    SavePreset(PresetName),
    RemovePreset(PresetName),
    Region {
        start: (u8, u8),
        end: (u8, u8),
        zoom_out: bool,
    },
    /// Image point in 0..255; x from left, y from bottom. A fixed lens pans/tilts;
    /// this never invents an optical zoom request.
    Point {
        x: u8,
        y: u8,
    },
    /// Ordinary release follows the configured deceleration.
    Release,
    /// Urgent cancellation bypasses the ramp and queued motion.
    Stop,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Wire {
    Position3d {
        v: u8,
        start_x: u8,
        start_y: u8,
        end_x: u8,
        end_y: u8,
        #[serde(default = "zoomto")]
        operation: String,
    },
    Home {
        v: u8,
    },
    Preset {
        v: u8,
        name: String,
    },
    SavePreset {
        v: u8,
        name: String,
    },
    RemovePreset {
        v: u8,
        name: String,
    },
    Velocity {
        v: u8,
        #[serde(default)]
        pan: i16,
        #[serde(default)]
        tilt: i16,
        #[serde(default)]
        zoom: i16,
    },
    Point {
        v: u8,
        x: u8,
        y: u8,
    },
    Release {
        v: u8,
    },
    Stop {
        v: u8,
    },
}

fn zoomto() -> String {
    "zoomto".into()
}
impl Request {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() > MAX_PAYLOAD {
            return Err("motion payload too large".into());
        }
        let wire: Wire = serde_json::from_str(text).map_err(|_| "invalid motion payload")?;
        let (version, request) = match wire {
            Wire::Position3d {
                v,
                start_x,
                start_y,
                end_x,
                end_y,
                operation,
            } => {
                let zoom_out = match operation.as_str() {
                    "zoomto" => false,
                    "zoomout" => true,
                    _ => return Err("invalid position3d operation".into()),
                };
                (
                    v,
                    Self::Region {
                        start: (start_x, start_y),
                        end: (end_x, end_y),
                        zoom_out,
                    },
                )
            }
            Wire::Home { v } => (v, Self::Home),
            Wire::Preset { v, name } => (v, Self::Preset(PresetName::parse(&name)?)),
            Wire::SavePreset { v, name } => (v, Self::SavePreset(PresetName::parse(&name)?)),
            Wire::RemovePreset { v, name } => (v, Self::RemovePreset(PresetName::parse(&name)?)),
            Wire::Velocity { v, pan, tilt, zoom } => {
                let velocity = Velocity { pan, tilt, zoom }.validate()?;
                (
                    v,
                    if velocity.is_zero() {
                        Self::Release
                    } else {
                        Self::Velocity(velocity)
                    },
                )
            }
            Wire::Point { v, x, y } => (v, Self::Point { x, y }),
            Wire::Release { v } => (v, Self::Release),
            Wire::Stop { v } => (v, Self::Stop),
        };
        if version != VERSION {
            return Err("unsupported motion version".into());
        }
        Ok(request)
    }
}

/// The adapter validates the whole request before actuating any axis. A fixed
/// lens never partially executes pan from a packet also requesting zoom.
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    pub zoom: bool,
    pub coupled_pan_tilt: bool,
}
impl Capabilities {
    pub fn validate(self, velocity: Velocity) -> Result<Velocity, String> {
        let velocity = velocity.validate()?;
        if velocity.zoom != 0 && !self.zoom {
            return Err("optical zoom unavailable".into());
        }
        if self.coupled_pan_tilt
            && velocity.pan != 0
            && velocity.tilt != 0
            && velocity.pan.abs() != velocity.tilt.abs()
        {
            return Err("pan and tilt share one speed clock".into());
        }
        Ok(velocity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_combinations_use_one_complete_vector() {
        for pan in [-100, 0, 100] {
            for tilt in [-100, 0, 100] {
                for zoom in [-100, 0, 100] {
                    let text = format!(
                        r#"{{"v":1,"op":"velocity","pan":{pan},"tilt":{tilt},"zoom":{zoom}}}"#
                    );
                    let expected = Velocity { pan, tilt, zoom };
                    assert_eq!(
                        Request::parse(&text).unwrap(),
                        if expected.is_zero() {
                            Request::Release
                        } else {
                            Request::Velocity(expected)
                        }
                    );
                }
            }
        }
        assert_eq!(
            Request::parse(r#"{"v":1,"op":"velocity","pan":-35}"#).unwrap(),
            Request::Velocity(Velocity {
                pan: -35,
                ..Velocity::default()
            })
        );
        assert_eq!(
            Request::parse(r#"{"v":1,"op":"point","x":200,"y":150}"#).unwrap(),
            Request::Point { x: 200, y: 150 }
        );
        assert_eq!(
            Request::parse(r#"{"v":1,"op":"remove_preset","name":"Door"}"#).unwrap(),
            Request::RemovePreset(PresetName::parse("Door").unwrap())
        );
    }
    #[test]
    fn invalid_messages_never_become_partial_motion() {
        for text in [
            r#"{"op":"velocity","pan":100}"#,
            r#"{"v":2,"op":"release"}"#,
            r#"{"v":1,"op":"velocity","pan":101}"#,
            r#"{"v":1,"op":"velocity","pan":0.5}"#,
            r#"{"v":1,"op":"stop","pan":100}"#,
            r#"{"v":1,"op":"point","x":256,"y":0}"#,
            r#"{"v":1,"op":"velocity","pan":10,"pan":20}"#,
            r#"{"v":1,"op":"velocity","focus":1}"#,
        ] {
            assert!(Request::parse(text).is_err(), "{text}");
        }
        let fixed = Capabilities {
            zoom: false,
            coupled_pan_tilt: true,
        };
        assert!(
            fixed
                .validate(Velocity {
                    pan: 100,
                    zoom: 1,
                    ..Velocity::default()
                })
                .is_err()
        );
        assert!(
            fixed
                .validate(Velocity {
                    pan: 100,
                    tilt: 50,
                    ..Velocity::default()
                })
                .is_err()
        );
        assert!(
            fixed
                .validate(Velocity {
                    pan: 100,
                    tilt: -100,
                    ..Velocity::default()
                })
                .is_ok()
        );
        assert!(
            Capabilities {
                zoom: true,
                coupled_pan_tilt: false
            }
            .validate(Velocity {
                pan: 30,
                tilt: 70,
                zoom: 100
            })
            .is_ok()
        );
    }
}
