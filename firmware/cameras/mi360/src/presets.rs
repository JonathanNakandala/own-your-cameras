//! Referenced pan/tilt positions. Persist only real stopped captures; a saved
//! position is usable only after this boot establishes the physical home frame.
use crate::board::{MotorStatus, Position};
use camera_transport::motion_control::PresetName;
use camera_transport::settings::{Disk, FileDisk};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Preset {
    pan: i32,
    tilt: i32,
    #[serde(default = "default_approach")]
    approach: [i8; 2],
}
impl Preset {
    fn valid(self) -> bool {
        (0..=crate::board::PAN_TRAVEL).contains(&self.pan)
            && (0..=crate::board::TILT_TRAVEL).contains(&self.tilt)
            && self.approach.iter().all(|v| matches!(v, -1 | 1))
    }
    fn position(self) -> Position {
        Position {
            x: self.pan,
            y: self.tilt,
        }
    }
}

fn default_approach() -> [i8; 2] {
    [1, 1]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    pub waypoint: Position,
    pub target: Position,
}
impl Preset {
    fn route(self) -> Route {
        let backoff =
            |at: i32, direction: i8, max: i32| (at - i32::from(direction) * 64).clamp(0, max);
        Route {
            waypoint: Position {
                x: backoff(self.pan, self.approach[0], crate::board::PAN_TRAVEL),
                y: backoff(self.tilt, self.approach[1], crate::board::TILT_TRAVEL),
            },
            target: self.position(),
        }
    }
}

/// Track actual step directions, retaining the last direction of idle axes.
/// This is gear take-up history, not an encoder or a claim about physical pose.
pub struct Approach {
    at: Position,
    directions: [i8; 2],
}
impl Approach {
    pub fn homed(at: Position) -> Self {
        Self {
            at,
            directions: default_approach(),
        }
    }
    pub fn observe(&mut self, at: Position) {
        for (i, (before, after)) in [(self.at.x, at.x), (self.at.y, at.y)]
            .into_iter()
            .enumerate()
        {
            if before != after {
                self.directions[i] = if after > before { 1 } else { -1 };
            }
        }
        self.at = at;
    }
    pub fn directions(&self) -> [i8; 2] {
        self.directions
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u8,
    positions: BTreeMap<String, Preset>,
}
pub struct Presets {
    store: FileDisk,
    positions: BTreeMap<String, Preset>,
}
impl Presets {
    pub fn load(path: &str) -> io::Result<Self> {
        let mut store = FileDisk::new(path).private_dir();
        let positions = match store.read()? {
            None => BTreeMap::new(),
            Some(bytes) => Self::decode(&bytes)?,
        };
        Ok(Self { store, positions })
    }
    fn decode(bytes: &[u8]) -> io::Result<BTreeMap<String, Preset>> {
        let doc: Document = serde_json::from_slice(bytes).map_err(io::Error::other)?;
        if !matches!(doc.version, 1 | 2)
            || doc.positions.len() > 16
            || doc
                .positions
                .iter()
                .any(|(k, p)| PresetName::parse(k).is_err() || !p.valid())
        {
            return Err(io::Error::other("invalid preset document"));
        }
        Ok(doc.positions)
    }
    pub fn get(&self, name: PresetName, referenced: bool) -> io::Result<Route> {
        if !referenced {
            return Err(io::Error::other("home camera before recalling presets"));
        }
        self.positions
            .get(name.as_str().map_err(io::Error::other)?)
            .copied()
            .map(Preset::route)
            .ok_or_else(|| io::Error::other("preset is not saved"))
    }
    pub fn save(
        &mut self,
        name: PresetName,
        status: MotorStatus,
        referenced: bool,
        approach: [i8; 2],
    ) -> io::Result<()> {
        if !referenced || status.moving || status.owner_open {
            return Err(io::Error::other(
                "preset capture needs a homed, stopped camera",
            ));
        }
        let p = Preset {
            pan: status.position.x,
            tilt: status.position.y,
            approach,
        };
        if !p.valid() {
            return Err(io::Error::other("preset outside travel"));
        }
        let mut positions = self.positions.clone();
        positions.insert(name.as_str().map_err(io::Error::other)?.into(), p);
        if positions.len() > 16 {
            return Err(io::Error::other("preset table is full"));
        }
        let bytes = serde_json::to_vec(&Document {
            version: 2,
            positions: positions.clone(),
        })
        .map_err(io::Error::other)?;
        self.store.write(&bytes)?;
        self.positions = positions;
        Ok(())
    }
    /// The saved names, in order.
    pub fn names(&self) -> Vec<String> {
        self.positions.keys().cloned().collect()
    }
    /// Forgets `name` and persists the table; an unknown name is an error.
    pub fn remove(&mut self, name: PresetName) -> io::Result<()> {
        let mut positions = self.positions.clone();
        if positions
            .remove(name.as_str().map_err(io::Error::other)?)
            .is_none()
        {
            return Err(io::Error::other("preset is not saved"));
        }
        let bytes = serde_json::to_vec(&Document {
            version: 2,
            positions: positions.clone(),
        })
        .map_err(io::Error::other)?;
        self.store.write(&bytes)?;
        self.positions = positions;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn records_require_supported_frame_and_bounded_positions() {
        assert!(
            Presets::decode(br#"{"version":1,"positions":{"1":{"pan":100,"tilt":200}}}"#).is_ok()
        );
        for bytes in [
            br#"{"version":3,"positions":{}}"#.as_slice(),
            br#"{"version":1,"positions":{"1":{"pan":2405,"tilt":0}}}"#,
            br#"{"version":1,"positions":{"1":{"pan":0,"tilt":-1}}}"#,
        ] {
            assert!(Presets::decode(bytes).is_err());
        }
        let p = Presets {
            store: FileDisk::new("/does-not-exist/presets.json").private_dir(),
            positions: BTreeMap::from([(
                "1".into(),
                Preset {
                    pan: 100,
                    tilt: 200,
                    approach: [1, -1],
                },
            )]),
        };
        let key = PresetName::parse("1").unwrap();
        assert!(p.get(key, false).is_err());
        assert_eq!(
            p.get(key, true).unwrap(),
            Route {
                waypoint: Position { x: 36, y: 264 },
                target: Position { x: 100, y: 200 }
            }
        );
        assert!(p.get(PresetName::parse("2").unwrap(), true).is_err());
    }
    #[test]
    fn approach_history_and_routes_preserve_saved_gear_loading() {
        let mut a = Approach::homed(Position { x: 1202, y: 356 });
        a.observe(Position { x: 900, y: 356 });
        assert_eq!(a.directions(), [-1, 1]);
        a.observe(Position { x: 900, y: 200 });
        assert_eq!(a.directions(), [-1, -1]);
        let p = Preset {
            pan: 900,
            tilt: 200,
            approach: a.directions(),
        };
        assert_eq!(p.route().waypoint, Position { x: 964, y: 264 });
        let p = Preset {
            pan: 0,
            tilt: crate::board::TILT_TRAVEL,
            approach: [-1, 1],
        };
        assert_eq!(
            p.route().waypoint,
            Position {
                x: 64,
                y: crate::board::TILT_TRAVEL - 64
            }
        );
        assert!(
            !Preset {
                approach: [0, 1],
                ..p
            }
            .valid()
        );
    }
}
