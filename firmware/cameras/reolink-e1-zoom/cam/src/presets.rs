//! The preset table the hardware thread moves to, saves into and removes from: `cam.toml`'s presets with the
//! daemon-owned overlay file applied by name. The overlay holds saved positions (over a `cam.toml` preset or
//! for a preset created at runtime) and removals of `cam.toml` presets. `cam.toml` is never written; a hand
//! edit of a preset there is newer than an earlier save or removal and wins over it.
use crate::settings::Disk;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const MAX_FILE: usize = 16 * 1024;
/// Presets in force at most, `cam.toml`'s and created ones together (also `cam.toml`'s own limit).
pub const MAX_PRESETS: usize = 32;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Preset {
    pub pan: u32,
    pub tilt: u32,
    pub zoom: u32,
}

/// A preset's name is 1–64 bytes and its position lies within travel (`cam.toml` and the overlay alike).
pub fn check(name: &str, p: &Preset) -> Result<(), String> {
    if name.len() > 64
        || name.trim().is_empty()
        || p.pan > crate::ptz::PAN_MAX
        || p.tilt > crate::ptz::TILT_MAX
        || p.zoom > crate::mqtt::ZOOM_MAX
    {
        return Err("invalid preset name or position".into());
    }
    Ok(())
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One overlay entry: a saved position and the `cam.toml` position it replaced (none for a preset created at
/// runtime), or the removal of a `cam.toml` preset (its position is then `base`'s).
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Entry {
    pan: u32,
    tilt: u32,
    zoom: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<Preset>,
    #[serde(default, skip_serializing_if = "is_false")]
    removed: bool,
}

impl Entry {
    fn at(&self) -> Preset {
        Preset {
            pan: self.pan,
            tilt: self.tilt,
            zoom: self.zoom,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    At(Preset),
    Removed,
}

pub struct Presets {
    base: BTreeMap<String, Preset>,
    /// Overlay changes in force: saved positions and removals of `base` names.
    overlay: BTreeMap<String, Change>,
    disk: Box<dyn Disk>,
}

impl Presets {
    /// `cam.toml`'s presets with the overlay on `disk` applied; never fails. An absent overlay is normal; an
    /// unreadable or invalid one is ignored as a whole. Returns the problems to log once.
    pub fn load(base: BTreeMap<String, Preset>, mut disk: Box<dyn Disk>) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let entries = match disk.read() {
            Ok(None) => BTreeMap::new(),
            Ok(Some(text)) => parse(&text).unwrap_or_else(|e| {
                problems.push(format!("saved presets ignored: {e}"));
                BTreeMap::new()
            }),
            Err(e) => {
                problems.push(format!("saved presets unreadable: {e}"));
                BTreeMap::new()
            }
        };
        let mut overlay = BTreeMap::new();
        for (name, e) in entries {
            let change = if e.removed {
                Change::Removed
            } else {
                Change::At(e.at())
            };
            match (base.get(&name), e.base) {
                (Some(b), Some(replaced)) if *b == replaced => {
                    overlay.insert(name, change);
                }
                (Some(_), Some(_)) => problems.push(format!(
                    "saved preset {name} ignored: edited in cam.toml since the save"
                )),
                (Some(_), None) => problems.push(format!(
                    "saved preset {name} ignored: cam.toml defines it since"
                )),
                (None, Some(_)) => {
                    problems.push(format!("saved preset {name} ignored: not in cam.toml"))
                }
                (None, None) if e.removed => {
                    problems.push(format!("removal of {name} ignored: not in cam.toml"))
                }
                (None, None) => {
                    overlay.insert(name, change);
                }
            }
        }
        let mut presets = Self {
            base,
            overlay,
            disk,
        };
        // A hand edit can push the total over the limit: created presets beyond it are dropped.
        while presets.names().len() > MAX_PRESETS {
            let Some(name) = presets
                .overlay
                .iter()
                .rev()
                .find(|(n, c)| matches!(c, Change::At(_)) && !presets.base.contains_key(*n))
                .map(|(n, _)| n.clone())
            else {
                break;
            };
            problems.push(format!(
                "saved preset {name} ignored: over {MAX_PRESETS} presets"
            ));
            presets.overlay.remove(&name);
        }
        (presets, problems)
    }

    /// The position in force for `name`: the saved one, else `cam.toml`'s; `None` once removed.
    pub fn get(&self, name: &str) -> Option<Preset> {
        match self.overlay.get(name) {
            Some(Change::At(p)) => Some(*p),
            Some(Change::Removed) => None,
            None => self.base.get(name).copied(),
        }
    }

    /// The presets in force, in name order.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .base
            .keys()
            .filter(|n| !matches!(self.overlay.get(*n), Some(Change::Removed)))
            .chain(
                self.overlay
                    .iter()
                    .filter(|(n, c)| matches!(c, Change::At(_)) && !self.base.contains_key(*n))
                    .map(|(n, _)| n),
            )
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// Writes `at` as `name`'s position to the overlay now, then puts it in force; a new name creates the
    /// preset while there is room. On an error nothing changes.
    pub fn save(&mut self, name: &str, at: Preset) -> Result<(), String> {
        check(name, &at)?;
        if self.get(name).is_none() && self.names().len() >= MAX_PRESETS {
            return Err(format!("preset table is full ({MAX_PRESETS})"));
        }
        let mut next = self.overlay.clone();
        next.insert(name.to_owned(), Change::At(at));
        self.commit(next)
    }

    /// Removes `name`: a created preset is forgotten, a `cam.toml` one is recorded as removed. On an error
    /// nothing changes.
    pub fn remove(&mut self, name: &str) -> Result<(), String> {
        if self.get(name).is_none() {
            return Err("unknown preset".into());
        }
        let mut next = self.overlay.clone();
        if self.base.contains_key(name) {
            next.insert(name.to_owned(), Change::Removed);
        } else {
            next.remove(name);
        }
        self.commit(next)
    }

    fn commit(&mut self, next: BTreeMap<String, Change>) -> Result<(), String> {
        let text = self.render(&next)?;
        self.write(text.as_bytes()).map_err(|e| e.to_string())?;
        self.overlay = next;
        Ok(())
    }

    fn render(&self, overlay: &BTreeMap<String, Change>) -> Result<String, String> {
        let entries: BTreeMap<&str, Entry> = overlay
            .iter()
            .filter_map(|(name, change)| {
                let base = self.base.get(name).copied();
                let (at, removed) = match change {
                    Change::At(p) => (*p, false),
                    Change::Removed => (base?, true),
                };
                Some((
                    name.as_str(),
                    Entry {
                        pan: at.pan,
                        tilt: at.tilt,
                        zoom: at.zoom,
                        base,
                        removed,
                    },
                ))
            })
            .collect();
        let text = format!(
            "# Written by cam: saved, created and removed presets. `base` is the cam.toml position a save or\n\
             # removal replaced; editing that preset in cam.toml makes the entry void.\n{}",
            toml::to_string(&entries).map_err(|e| e.to_string())?
        );
        if text.len() > MAX_FILE {
            return Err(format!("{} bytes, over the {MAX_FILE} limit", text.len()));
        }
        Ok(text)
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.disk.write_temp(bytes)?;
        self.disk.sync_temp()?;
        self.disk.rename_temp()?;
        self.disk.sync_dir()
    }
}

fn parse(text: &str) -> Result<BTreeMap<String, Entry>, String> {
    if text.len() > MAX_FILE {
        return Err(format!("{} bytes, over the {MAX_FILE} limit", text.len()));
    }
    // TOML errors quote input lines; keep only the fact.
    let entries: BTreeMap<String, Entry> =
        toml::from_str(text).map_err(|_| "not a valid presets file".to_string())?;
    for (name, e) in &entries {
        check(name, &e.at())?;
        if let Some(base) = &e.base {
            check(name, base)?;
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::FakeDisk;
    use std::sync::{Arc, Mutex};

    /// A fake disk the test keeps a handle on after `Presets` took it.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<FakeDisk>>);
    impl Shared {
        fn with(text: &str) -> Self {
            let s = Self::default();
            s.0.lock().unwrap().file = Some(text.into());
            s
        }
        fn file(&self) -> Option<String> {
            self.0.lock().unwrap().file.clone()
        }
    }
    impl Disk for Shared {
        fn read(&mut self) -> std::io::Result<Option<String>> {
            self.0.lock().unwrap().read()
        }
        fn write_temp(&mut self, b: &[u8]) -> std::io::Result<()> {
            self.0.lock().unwrap().write_temp(b)
        }
        fn sync_temp(&mut self) -> std::io::Result<()> {
            self.0.lock().unwrap().sync_temp()
        }
        fn rename_temp(&mut self) -> std::io::Result<()> {
            self.0.lock().unwrap().rename_temp()
        }
        fn sync_dir(&mut self) -> std::io::Result<()> {
            self.0.lock().unwrap().sync_dir()
        }
    }

    fn p(pan: u32, tilt: u32, zoom: u32) -> Preset {
        Preset { pan, tilt, zoom }
    }

    fn base() -> BTreeMap<String, Preset> {
        [
            ("Door".into(), p(6195, 650, 0)),
            ("Desk".into(), p(100, 200, 300)),
        ]
        .into()
    }

    #[test]
    fn without_an_overlay_the_positions_are_cam_toml_s() {
        let disk = Shared::default();
        let (presets, problems) = Presets::load(base(), Box::new(disk.clone()));
        assert!(problems.is_empty());
        assert_eq!(presets.get("Door"), Some(p(6195, 650, 0)));
        assert_eq!(presets.get("nosuch"), None);
        assert_eq!(disk.file(), None);
    }

    #[test]
    fn a_save_is_in_force_at_once_and_after_a_reload() {
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        presets.save("Door", p(6100, 700, 256)).unwrap();
        assert_eq!(presets.get("Door"), Some(p(6100, 700, 256)));
        assert_eq!(presets.get("Desk"), Some(p(100, 200, 300)));
        assert_eq!(
            disk.0.lock().unwrap().ops,
            ["read", "write_temp", "sync_temp", "rename_temp", "sync_dir"]
        );
        let text = disk.file().unwrap();
        assert!(text.contains("[Door]"), "{text}");
        let (again, problems) = Presets::load(base(), Box::new(disk));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(again.get("Door"), Some(p(6100, 700, 256)));
    }

    #[test]
    fn the_overlay_records_the_position_it_replaced() {
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        presets.save("Door", p(6100, 700, 256)).unwrap();
        let entries = parse(&disk.file().unwrap()).unwrap();
        assert_eq!(entries["Door"].base, Some(p(6195, 650, 0)));
        assert_eq!(entries["Door"].at(), p(6100, 700, 256));
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn a_newer_hand_edit_in_cam_toml_wins_and_drops_the_entry_at_the_next_write() {
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        presets.save("Door", p(6100, 700, 256)).unwrap();
        let mut edited = base();
        edited.insert("Door".into(), p(5000, 600, 100));
        let (mut presets, problems) = Presets::load(edited, Box::new(disk.clone()));
        assert_eq!(presets.get("Door"), Some(p(5000, 600, 100)));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("Door"));
        presets.save("Desk", p(1, 2, 3)).unwrap();
        let entries = parse(&disk.file().unwrap()).unwrap();
        assert_eq!(entries.keys().collect::<Vec<_>>(), ["Desk"]);
    }

    #[test]
    fn an_entry_for_a_removed_name_is_ignored() {
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        presets.save("Door", p(6100, 700, 256)).unwrap();
        let mut fewer = base();
        fewer.remove("Door");
        let (presets, problems) = Presets::load(fewer, Box::new(disk));
        assert_eq!(presets.get("Door"), None);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("not in cam.toml"), "{problems:?}");
    }

    #[test]
    fn an_invalid_overlay_is_ignored_as_a_whole_with_one_problem() {
        for text in [
            "not toml [",
            "[Door]\npan = 1\ntilt = 1\nzoom = 1\nbase = { pan = 6195, tilt = 650, zoom = 0 }\nfocus = 3\n",
            "[Door]\npan = 999999\ntilt = 1\nzoom = 1\nbase = { pan = 6195, tilt = 650, zoom = 0 }\n",
            "[Desk]\npan = 1\ntilt = 1\nzoom = 1\nbase = { pan = 100, tilt = 200, zoom = 300 }\n\
             [Door]\npan = 1\ntilt = 1\nzoom = 99999\nbase = { pan = 6195, tilt = 650, zoom = 0 }\n",
        ] {
            let (presets, problems) = Presets::load(base(), Box::new(Shared::with(text)));
            assert_eq!(problems.len(), 1, "{text}: {problems:?}");
            assert_eq!(presets.get("Door"), Some(p(6195, 650, 0)), "{text}");
            assert_eq!(presets.get("Desk"), Some(p(100, 200, 300)), "{text}");
        }
    }

    #[test]
    fn an_unreadable_overlay_is_one_problem() {
        let disk = Shared::default();
        disk.0.lock().unwrap().fail_read = true;
        let (presets, problems) = Presets::load(base(), Box::new(disk));
        assert_eq!(problems.len(), 1);
        assert_eq!(presets.get("Door"), Some(p(6195, 650, 0)));
    }

    #[test]
    fn an_overlay_over_16_kib_is_ignored() {
        let mut text = String::from(
            "[Door]\npan = 1\ntilt = 1\nzoom = 1\nbase = { pan = 6195, tilt = 650, zoom = 0 }\n",
        );
        text.push_str(&"#".repeat(MAX_FILE));
        let (presets, problems) = Presets::load(base(), Box::new(Shared::with(&text)));
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("limit"), "{problems:?}");
        assert_eq!(presets.get("Door"), Some(p(6195, 650, 0)));
    }

    #[test]
    fn a_failed_write_keeps_the_old_position() {
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        presets.save("Door", p(6100, 700, 256)).unwrap();
        disk.0.lock().unwrap().fail_write = true;
        assert!(presets.save("Door", p(1, 2, 3)).is_err());
        assert_eq!(presets.get("Door"), Some(p(6100, 700, 256)));
    }

    #[test]
    fn an_out_of_travel_position_or_a_bad_name_is_not_saved() {
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        assert!(presets.save("Door", p(u32::MAX, 2, 3)).is_err());
        assert!(presets.save(" ", p(1, 2, 3)).is_err());
        assert_eq!(disk.file(), None);
    }

    #[test]
    fn a_new_name_creates_a_preset_that_survives_a_reload() {
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        presets.save("Window", p(1, 2, 3)).unwrap();
        assert_eq!(presets.names(), ["Desk", "Door", "Window"]);
        let entries = parse(&disk.file().unwrap()).unwrap();
        assert_eq!(entries["Window"].base, None);
        let (again, problems) = Presets::load(base(), Box::new(disk));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(again.get("Window"), Some(p(1, 2, 3)));
    }

    #[test]
    fn removing_forgets_a_created_preset_and_records_a_cam_toml_one() {
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        presets.save("Window", p(1, 2, 3)).unwrap();
        presets.remove("Window").unwrap();
        presets.remove("Door").unwrap();
        assert_eq!(presets.get("Door"), None);
        assert_eq!(presets.names(), ["Desk"]);
        assert_eq!(presets.remove("Door"), Err("unknown preset".into()));
        let (mut again, problems) = Presets::load(base(), Box::new(disk.clone()));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(again.names(), ["Desk"]);
        // Saving the name again brings the preset back at the new position.
        again.save("Door", p(9, 9, 9)).unwrap();
        assert_eq!(again.get("Door"), Some(p(9, 9, 9)));
        // A hand edit of the removed preset in cam.toml wins over the removal.
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        presets.remove("Door").unwrap();
        let mut edited = base();
        edited.insert("Door".into(), p(5000, 600, 100));
        let (presets, problems) = Presets::load(edited, Box::new(disk));
        assert_eq!(presets.get("Door"), Some(p(5000, 600, 100)));
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn the_table_holds_at_most_32_presets() {
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        for i in 0..MAX_PRESETS - 2 {
            presets.save(&format!("p{i:02}"), p(1, 1, 1)).unwrap();
        }
        assert_eq!(presets.names().len(), MAX_PRESETS);
        assert!(presets.save("one more", p(1, 1, 1)).is_err());
        presets.save("Door", p(2, 2, 2)).unwrap();
        presets.remove("p00").unwrap();
        presets.save("one more", p(1, 1, 1)).unwrap();
    }

    #[test]
    fn a_created_preset_that_cam_toml_defines_later_is_cam_toml_s() {
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(base(), Box::new(disk.clone()));
        presets.save("Window", p(1, 2, 3)).unwrap();
        let mut more = base();
        more.insert("Window".into(), p(7, 7, 7));
        let (presets, problems) = Presets::load(more, Box::new(disk));
        assert_eq!(presets.get("Window"), Some(p(7, 7, 7)));
        assert_eq!(problems.len(), 1, "{problems:?}");
    }

    #[test]
    fn names_that_need_quoting_round_trip() {
        let mut b = base();
        b.insert("Living room \"sofa\"".into(), p(1, 1, 1));
        let disk = Shared::default();
        let (mut presets, _) = Presets::load(b.clone(), Box::new(disk.clone()));
        presets.save("Living room \"sofa\"", p(2, 2, 2)).unwrap();
        let (again, problems) = Presets::load(b, Box::new(disk));
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(again.get("Living room \"sofa\""), Some(p(2, 2, 2)));
    }
}
