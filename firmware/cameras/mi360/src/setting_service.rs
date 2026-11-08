//! Confirmed hardware settings, bounded queued changes and deferred persistence.
//! The media owner supplies hardware and time; this module never publishes MQTT.
//! Only values the hardware reported or confirmed enter the sparse [`Values`]: a boot readback is
//! observed (saved with the next change), an accepted change is put (starts the save window).
//! A failed save is retried one save window later.
//! At startup a saved value is published as loaded until its apply settles; the hardware's own
//! value from before the apply is never published.
use crate::media::ControlResult;
use crate::settings::{self, SPECS};
use camera_transport::mqtt::{Category, Domain, Entity};
use camera_transport::settings::{Codec, Disk, Field, JsonCodec, Spec, Values};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::time::Duration;

const DEADLINE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(250);

pub trait Hardware {
    fn get(&mut self, control: u32) -> io::Result<i32>;
    fn set(&mut self, control: u32, value: i32) -> io::Result<ControlResult>;
    fn software_get(&self, slot: u16) -> io::Result<i32>;
    fn software_set(&mut self, slot: u16, value: i32) -> io::Result<()>;
}

struct Pending {
    control: u32,
    requested: i32,
    previous: i32,
    deadline: Duration,
    next_poll: Duration,
    rollback: bool,
}

/// The values in force and their settings file.
pub type Store = Values<'static, Box<dyn Disk>, StrictJson>;

/// The shared JSON format, but as strict as Mi 360 always was: any value that is not an `i32`
/// (a boolean, a text, a fraction, a huge number) makes the whole document invalid.
#[derive(Debug, Default, Clone, Copy)]
pub struct StrictJson;
impl Codec for StrictJson {
    fn decode(&self, bytes: &[u8]) -> Result<BTreeMap<String, Field>, String> {
        let fields = JsonCodec.decode(bytes)?;
        let integer = |f: &Field| matches!(f, Field::Int(n) if i32::try_from(*n).is_ok());
        if fields.values().all(integer) {
            Ok(fields)
        } else {
            Err("invalid settings document".into())
        }
    }
    fn encode(&self, values: &[(&Spec, i32)]) -> Result<Vec<u8>, String> {
        JsonCodec.encode(values)
    }
}

pub struct Service {
    values: Store,
    available: BTreeSet<u16>,
    actual: BTreeMap<u16, i32>,
    pending: BTreeMap<u16, Pending>,
    startup: BTreeMap<u16, i32>,
    updates: BTreeMap<&'static str, String>,
    /// Saved slots whose startup apply has not settled: their readbacks are not published yet.
    held: BTreeSet<u16>,
    seen_healthy: bool,
    next_refresh: Duration,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn queued_channel(control: Option<u32>) -> Option<u32> {
    match control {
        Some(control @ (20 | 21 | 24..=27)) => Some(control & 1),
        _ => None,
    }
}

fn read(hw: &mut impl Hardware, spec: &Spec) -> io::Result<(i32, i32)> {
    let raw = match settings::hardware(spec.slot) {
        Some(control) => hw.get(control)?,
        None => hw.software_get(spec.slot)?,
    };
    if spec.slot <= 1 && !(0..=3).contains(&raw) {
        return Err(io::Error::other("invalid raw orientation readback"));
    }
    let value = settings::decode(spec.slot, raw);
    if !spec.valid(value) {
        return Err(io::Error::other("hardware readback outside catalog bounds"));
    }
    Ok((raw, value))
}

impl Service {
    pub fn initialize(
        hw: &mut impl Hardware,
        disk: Box<dyn Disk>,
        audio: bool,
        now: Duration,
    ) -> io::Result<Self> {
        // Unknown keys and out-of-range values are skipped alone. Malformed JSON, a
        // non-i32 value, document/version errors and unsafe filesystem metadata
        // fail startup.
        let (values, problems) = Values::load(SPECS, disk, StrictJson);
        for problem in &problems {
            if problem.key.is_none() {
                return Err(io::Error::other(format!("settings: {problem}")));
            }
            eprintln!("settings: {problem}; not restored");
        }
        Ok(Self::from_saved(hw, values.sparse(), audio, now))
    }

    /// `values` holds the saved values, and only those are set.
    fn from_saved(hw: &mut impl Hardware, values: Store, audio: bool, now: Duration) -> Self {
        let saved: BTreeMap<u16, i32> = SPECS
            .iter()
            .filter(|s| values.is_set(s.slot))
            .filter_map(|s| Some((s.slot, values.get(s.slot)?)))
            .collect();
        let mut service = Self {
            values,
            available: BTreeSet::new(),
            actual: BTreeMap::new(),
            pending: BTreeMap::new(),
            startup: BTreeMap::new(),
            updates: BTreeMap::new(),
            held: saved.keys().copied().collect(),
            seen_healthy: false,
            next_refresh: now,
        };
        // One shared HV snapshot keeps the two logical axes coherent.
        for d in SPECS {
            if settings::audio(d.slot) && !audio || d.slot == 1 {
                continue;
            }
            if let Ok((raw, value)) = read(hw, d) {
                service.available.insert(d.slot);
                let _ = service.values.observe(d.slot, value);
                if d.slot == 0 {
                    service.available.insert(1);
                    let _ = service.values.observe(1, settings::decode(1, raw));
                }
                service.observe(d, raw);
            }
        }
        // As before the shared store: a capability absent on this boot drops its saved value.
        for d in SPECS {
            if !service.available.contains(&d.slot) {
                service.values.forget(d.slot);
            }
        }
        service.held.retain(|slot| service.available.contains(slot));
        for &slot in &service.held {
            let d = settings::spec(slot).unwrap();
            service.updates.insert(d.key, d.render(saved[&slot]));
        }
        service.status(format!("ready: {} controls", service.available.len()));
        // Default is physical stock HV3, independently of discovery publication.
        // Missing saved axes become logical OFF; combine both into one write.
        if service.available.contains(&0) {
            let mirror = saved
                .get(&0)
                .copied()
                .filter(|v| (0..=1).contains(v))
                .unwrap_or(0);
            let flip = saved
                .get(&1)
                .copied()
                .filter(|v| (0..=1).contains(v))
                .unwrap_or(0);
            let raw = settings::STOCK_HV ^ mirror ^ (flip << 1);
            let _ = service.apply(0, mirror, Some(raw), hw, now);
        }
        for (slot, value) in saved {
            if slot > 1
                && service.available.contains(&slot)
                && let Err(error) = service.submit(slot, value, hw, now)
                && error.kind() == io::ErrorKind::WouldBlock
            {
                service.startup.insert(slot, value);
            }
        }
        service.settle();
        service
    }

    /// Publishes the value in force of every held slot whose apply finished (applied, rolled
    /// back or failed), and stops holding it.
    fn settle(&mut self) {
        let settled: Vec<u16> = self
            .held
            .iter()
            .copied()
            .filter(|slot| !self.pending.contains_key(slot) && !self.startup.contains_key(slot))
            .collect();
        for slot in settled {
            self.held.remove(&slot);
            let d = settings::spec(slot).unwrap();
            let value = self
                .actual
                .get(&slot)
                .map_or_else(|| "unknown".into(), |v| d.render(*v));
            self.updates.insert(d.key, value);
        }
    }

    pub fn entities(&self) -> Vec<Entity> {
        let mut result: Vec<_> = SPECS
            .iter()
            .filter(|d| self.available.contains(&d.slot))
            .map(Spec::entity)
            .collect();
        result.push(
            Entity::new("settings", "Settings status", Domain::Sensor)
                .category(Category::Diagnostic),
        );
        result
    }

    fn status(&mut self, message: String) {
        self.updates.insert("settings", message);
    }

    fn observe(&mut self, d: &Spec, raw: i32) {
        let slots: &[u16] = if d.slot <= 1 {
            &[0, 1]
        } else {
            std::slice::from_ref(&d.slot)
        };
        for slot in slots {
            if !self.available.contains(slot) {
                continue;
            }
            let spec = settings::spec(*slot).unwrap();
            let value = settings::decode(*slot, raw);
            if self.actual.insert(*slot, value) != Some(value) && !self.held.contains(slot) {
                self.updates.insert(spec.key, spec.render(value));
            }
        }
    }

    fn accept(&mut self, d: &Spec, raw: i32, now: Duration) {
        self.observe(d, raw);
        let slots: &[u16] = if d.slot <= 1 {
            &[0, 1]
        } else {
            std::slice::from_ref(&d.slot)
        };
        for &slot in slots {
            // Only a hardware-confirmed value is saved; an invalid one changes nothing.
            let _ = self.values.put(slot, settings::decode(slot, raw), now);
        }
    }

    /// The saved (or to-be-saved) hardware-confirmed value of a slot.
    #[cfg(test)]
    fn confirmed(&self, slot: u16) -> Option<i32> {
        self.values
            .is_set(slot)
            .then(|| self.values.get(slot))
            .flatten()
    }

    fn rejected_readback(&mut self, d: &Spec, hw: &mut impl Hardware) {
        // Real changed state is publishable even after a failed mutation. It is
        // deliberately separate from the persistable confirmed snapshot.
        if let Ok((raw, _)) = read(hw, d) {
            self.observe(d, raw);
        }
    }

    fn refresh_gop(&mut self, control: u32, hw: &mut impl Hardware) {
        let Some(d) = settings::slot_of(control).and_then(settings::spec) else {
            return;
        };
        if !self.available.contains(&d.slot) {
            return;
        }
        if let Ok((raw, _)) = read(hw, d) {
            self.observe(d, raw);
        } else if self.actual.remove(&d.slot).is_some() && !self.held.contains(&d.slot) {
            // An unsupported GOP record or nonintegral GOP/FPS duration has no
            // scalar value. Retaining an old number would misrepresent readback.
            self.updates.insert(d.key, "unknown".into());
        }
    }

    fn refresh_after_fps(&mut self, control: Option<u32>, hw: &mut impl Hardware) {
        if let Some(control @ (20 | 21)) = control {
            self.refresh_gop(control + 4, hw);
        }
    }

    pub fn request(
        &mut self,
        slot: u16,
        value: i32,
        hw: &mut impl Hardware,
        now: Duration,
    ) -> io::Result<()> {
        let result = self.submit(slot, value, hw, now);
        self.settle();
        result
    }

    fn submit(
        &mut self,
        slot: u16,
        value: i32,
        hw: &mut impl Hardware,
        now: Duration,
    ) -> io::Result<()> {
        if settings::spec(slot).is_some_and(|d| d.valid(value)) {
            // An explicit command supersedes an older deferred startup value.
            self.startup.remove(&slot);
        }
        self.apply(slot, value, None, hw, now)
    }

    fn apply(
        &mut self,
        slot: u16,
        value: i32,
        forced_raw: Option<i32>,
        hw: &mut impl Hardware,
        now: Duration,
    ) -> io::Result<()> {
        let d = settings::spec(slot).ok_or_else(|| invalid("unknown setting slot"))?;
        let hardware = settings::hardware(slot);
        if !self.available.contains(&slot) || !d.valid(value) {
            self.status(format!("rejected: {} unavailable or out of range", d.key));
            return Err(invalid("setting unavailable or out of range"));
        }
        let channel = queued_channel(hardware);
        if self.pending.contains_key(&slot)
            || channel.is_some_and(|channel| {
                self.pending
                    .values()
                    .any(|p| queued_channel(Some(p.control)) == Some(channel))
            })
        {
            self.status(format!("busy: {} pending", d.key));
            return Err(io::Error::new(io::ErrorKind::WouldBlock, "setting pending"));
        }
        let (previous, _) = read(hw, d).inspect_err(|_| {
            self.status(format!("rejected: {} readback failed", d.key));
        })?;
        self.observe(d, previous);
        let target = forced_raw.unwrap_or_else(|| settings::encode(slot, value, previous));
        if previous == target {
            self.accept(d, previous, now);
            self.refresh_after_fps(hardware, hw);
            return Ok(());
        }
        let result = match hardware {
            Some(control) => hw.set(control, target),
            None => hw
                .software_set(slot, target)
                .map(|()| ControlResult::Applied),
        };
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                self.rejected_readback(d, hw);
                self.status(format!("rejected: {} hardware error", d.key));
                return Err(error);
            }
        };
        let after = read(hw, d);
        if let Ok((raw, _)) = after {
            self.observe(d, raw);
            if raw == target && result != ControlResult::Unverified {
                self.accept(d, raw, now);
                self.refresh_after_fps(hardware, hw);
                self.status(format!("applied: {}", d.key));
                return Ok(());
            }
        }
        // Exact queued FPS/GOP/multiplier capabilities have bounded rollback.
        if let Some(control) = hardware.filter(|c| queued_channel(Some(*c)).is_some())
            && result != ControlResult::Unverified
        {
            self.pending.insert(
                slot,
                Pending {
                    control,
                    requested: target,
                    previous,
                    deadline: now.saturating_add(DEADLINE),
                    next_poll: now.saturating_add(POLL),
                    rollback: false,
                },
            );
            self.status(format!("pending: {}", d.key));
            return Ok(());
        }
        self.status(format!("rejected: {} no matching readback", d.key));
        Err(io::Error::other(
            "setting lacks matching confirmed readback",
        ))
    }

    pub fn tick(&mut self, hw: &mut impl Hardware, now: Duration, healthy: bool) -> io::Result<()> {
        self.seen_healthy |= healthy;
        let due: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.next_poll || now >= p.deadline)
            .map(|(slot, _)| *slot)
            .collect();
        for slot in due {
            let mut p = self.pending.remove(&slot).unwrap();
            let d = settings::spec(slot).unwrap();
            let actual = read(hw, d).ok().map(|(raw, _)| raw);
            if let Some(raw) = actual {
                self.observe(d, raw);
                if raw == p.requested && now <= p.deadline {
                    if !p.rollback {
                        self.accept(d, raw, now);
                    }
                    self.refresh_after_fps(settings::hardware(slot), hw);
                    self.status(format!(
                        "{}: {}",
                        if p.rollback { "rolled back" } else { "applied" },
                        d.key
                    ));
                    continue;
                }
            }
            if now < p.deadline {
                p.next_poll = now.saturating_add(POLL);
                self.pending.insert(slot, p);
                continue;
            }
            if p.rollback {
                self.status(format!("unconfirmed rollback: {}", d.key));
                continue;
            }
            // One rollback command, then at most five further seconds of reads.
            // No retry commands and no persistence of the failed target.
            match hw.set(p.control, p.previous) {
                Ok(result) if result != ControlResult::Unverified => {
                    p.requested = p.previous;
                    p.rollback = true;
                    p.deadline = now.saturating_add(DEADLINE);
                    p.next_poll = now;
                    self.pending.insert(slot, p);
                    self.status(format!("rolling back: {}", d.key));
                }
                _ => {
                    self.rejected_readback(d, hw);
                    self.status(format!("rollback failed: {}", d.key));
                }
            }
        }
        // FPS and GOP are coupled inside the SDK worker. A one-second refresh
        // also catches the interval between its FPS and GOP updates, without
        // persisting a transient or failed requested duration.
        if now >= self.next_refresh {
            self.next_refresh = now.saturating_add(Duration::from_secs(1));
            self.refresh_gop(24, hw);
            self.refresh_gop(25, hw);
            self.refresh_gop(26, hw);
            self.refresh_gop(27, hw);
        }
        let ready: Vec<_> = self
            .startup
            .iter()
            .filter_map(|(slot, value)| {
                let channel = queued_channel(settings::hardware(*slot));
                let blocked = channel.is_some_and(|channel| {
                    self.pending
                        .values()
                        .any(|p| queued_channel(Some(p.control)) == Some(channel))
                });
                (!blocked).then_some((*slot, *value))
            })
            .collect();
        for (slot, value) in ready {
            self.startup.remove(&slot);
            if let Err(error) = self.submit(slot, value, hw, now)
                && error.kind() == io::ErrorKind::WouldBlock
            {
                self.startup.insert(slot, value);
            }
        }
        self.settle();
        // A failed write is retried one save window later.
        if healthy && self.values.due(now) {
            let result = self.values.tick(now);
            self.saved(result)?;
        }
        Ok(())
    }

    fn saved(&mut self, result: io::Result<()>) -> io::Result<()> {
        match result {
            Ok(()) => self.status("saved".into()),
            Err(_) => self.status("save failed: confirmed settings remain dirty".into()),
        }
        result
    }

    pub fn take_updates(&mut self) -> Vec<(&'static str, String)> {
        std::mem::take(&mut self.updates).into_iter().collect()
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if self.seen_healthy && self.values.dirty() {
            let result = self.values.flush();
            self.saved(result)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        values: BTreeMap<u32, i32>,
        writes: Vec<(u32, i32)>,
        queued: bool,
        fail_set: bool,
        fail_get: bool,
        unverified: bool,
        speed: i32,
        software: BTreeMap<u16, i32>,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                values: BTreeMap::from([
                    (0, 3),
                    (1, 128),
                    (20, 20),
                    (24, 1),
                    (26, 3),
                    (27, 3),
                    (10, 55),
                ]),
                speed: 300,
                software: BTreeMap::from([
                    (settings::START_SPEED, 100),
                    (settings::RAMP_UP, 250),
                    (settings::RAMP_DOWN, 150),
                ]),
                ..Self::default()
            }
        }
    }
    impl Hardware for Fake {
        fn get(&mut self, control: u32) -> io::Result<i32> {
            if self.fail_get {
                return Err(io::Error::other("fixture read failure"));
            }
            self.values
                .get(&control)
                .copied()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "fixture unavailable"))
        }
        fn set(&mut self, control: u32, value: i32) -> io::Result<ControlResult> {
            self.writes.push((control, value));
            if !self.queued {
                self.values.insert(control, value);
            }
            if self.fail_set {
                return Err(io::Error::other("fixture mutation failed after write"));
            }
            Ok(if self.unverified {
                ControlResult::Unverified
            } else if self.queued {
                ControlResult::Pending
            } else {
                ControlResult::Applied
            })
        }
        fn software_get(&self, slot: u16) -> io::Result<i32> {
            if slot == settings::SPEED {
                return Ok(self.speed);
            }
            self.software
                .get(&slot)
                .copied()
                .ok_or_else(|| invalid("unknown software setting"))
        }
        fn software_set(&mut self, slot: u16, value: i32) -> io::Result<()> {
            if slot == settings::SPEED {
                self.speed = value;
            } else {
                *self
                    .software
                    .get_mut(&slot)
                    .ok_or_else(|| invalid("unknown software setting"))? = value;
            }
            Ok(())
        }
    }
    /// No file: every write fails, which catches accidental saves; persistence
    /// itself is covered by the `settings` and `camera_transport::settings` tests.
    struct Unusable;
    impl Disk for Unusable {
        fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn write(&mut self, _: &[u8]) -> io::Result<()> {
            Err(io::Error::other("isolated test store"))
        }
    }
    fn service(hw: &mut Fake, saved: BTreeMap<u16, i32>, audio: bool) -> Service {
        let disk: Box<dyn Disk> = Box::new(Unusable);
        let mut values = Values::load(SPECS, disk, StrictJson).0.sparse();
        for (slot, value) in saved {
            values.observe(slot, value).unwrap();
        }
        Service::from_saved(hw, values, audio, Duration::ZERO)
    }
    fn at(seconds: u64) -> Duration {
        Duration::from_secs(seconds)
    }

    struct File(Option<&'static str>);
    impl Disk for File {
        fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
            Ok(self.0.map(|t| t.as_bytes().to_vec()))
        }
        fn write(&mut self, _: &[u8]) -> io::Result<()> {
            Err(io::Error::other("isolated test store"))
        }
    }
    #[test]
    fn a_bad_document_fails_startup_and_a_bad_field_is_skipped_alone() {
        for bad in [
            r#"{"version":2,"values":{}}"#,
            "{not json",
            r#"{"version":1,"values":{"mirror":true}}"#,
            r#"{"version":1,"values":{"white_balance":"cloudy"}}"#,
            r#"{"version":1,"values":{"brightness":1.5}}"#,
            r#"{"version":1,"values":{"retired":4294967296}}"#,
        ] {
            let mut hw = Fake::new();
            assert!(
                Service::initialize(&mut hw, Box::new(File(Some(bad))), false, at(0)).is_err(),
                "{bad}"
            );
        }
        let mut hw = Fake::new();
        let ok = r#"{"version":1,"values":{"brightness":300,"retired":1,"ptz_ramp_up":800}}"#;
        let s = Service::initialize(&mut hw, Box::new(File(Some(ok))), false, at(0)).unwrap();
        assert_eq!(s.confirmed(settings::RAMP_UP), Some(800));
        assert_eq!(
            s.confirmed(2),
            Some(128),
            "the hardware value, not the out-of-range one"
        );
    }
    #[test]
    fn saved_values_of_capabilities_absent_on_this_boot_are_dropped() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::from([(3, 90), (8, 60)]), false);
        assert_eq!((s.confirmed(3), s.confirmed(8)), (None, None));
        s.request(2, 129, &mut hw, at(1)).unwrap();
        assert!(s.values.dirty());
    }
    #[test]
    fn motion_settings_restore_publish_and_persist_only_valid_accepted_values() {
        let mut hw = Fake::new();
        let saved = BTreeMap::from([
            (settings::START_SPEED, 300),
            (settings::RAMP_UP, 800),
            (settings::RAMP_DOWN, 0),
        ]);
        let mut s = service(&mut hw, saved.clone(), false);
        for (slot, value) in saved {
            let d = settings::spec(slot).unwrap();
            assert_eq!(s.confirmed(slot), Some(value));
            assert_eq!(hw.software_get(slot).unwrap(), value);
            let e = s.entities().into_iter().find(|e| e.id == d.key).unwrap();
            assert_eq!(e.control, camera_transport::mqtt::Control::Slot(slot));
        }
        s.request(settings::RAMP_DOWN, 3000, &mut hw, at(1))
            .unwrap();
        assert_eq!(s.confirmed(settings::RAMP_DOWN), Some(3000));
        for (slot, bad) in [
            (settings::START_SPEED, 99),
            (settings::RAMP_UP, -1),
            (settings::RAMP_DOWN, 3001),
        ] {
            let before = hw.software_get(slot).unwrap();
            assert!(s.request(slot, bad, &mut hw, at(2)).is_err());
            assert_eq!(hw.software_get(slot).unwrap(), before);
        }
    }
    #[test]
    fn capabilities_are_probed_and_audio_is_gated() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        let ids: Vec<_> = s.entities().into_iter().map(|e| e.id).collect();
        assert!(ids.contains(&"brightness".into()) && ids.contains(&"mirror".into()));
        assert!(!ids.contains(&"contrast".into()) && !ids.contains(&"mic_volume".into()));
        let status = s
            .entities()
            .into_iter()
            .find(|e| e.id == "settings")
            .unwrap();
        assert!(
            status.category == Some(Category::Diagnostic)
                && status.control == camera_transport::mqtt::Control::None
        );
        assert!(s.request(8, 60, &mut hw, at(1)).is_err());
        assert!(
            service(&mut hw, BTreeMap::new(), true)
                .available
                .contains(&8)
        );
    }
    #[test]
    fn white_balance_presets_encode_sdk_modes_and_persist_logical_indices_only() {
        let mut hw = Fake::new();
        hw.values.insert(35, 2); // SDK daylight mode maps to select index1.
        let mut s = service(&mut hw, BTreeMap::from([(23, 2)]), false);
        assert_eq!(hw.values[&35], 3); // saved select index2 maps to SDK cloudy3
        assert_eq!(s.actual.get(&23), Some(&2));
        assert_eq!(s.confirmed(23), Some(2));
        s.request(23, 7, &mut hw, at(1)).unwrap();
        assert_eq!(hw.values[&35], 8);
        assert_eq!(s.confirmed(23), Some(7));
        hw.values.insert(35, 1); // Manual gains are deliberately not restored.
        let s = service(&mut hw, BTreeMap::new(), false);
        assert!(!s.available.contains(&23));
    }
    #[test]
    fn startup_defaults_and_saved_orientation_are_combined_once() {
        let mut hw = Fake::new();
        hw.values.insert(0, 0);
        let mut s = service(&mut hw, BTreeMap::new(), false);
        assert_eq!(hw.writes, vec![(0, 3)]);
        assert_eq!(s.actual.get(&0), Some(&0));
        s.request(0, 1, &mut hw, at(1)).unwrap();
        assert_eq!(hw.values[&0], 2);
        s.request(1, 1, &mut hw, at(2)).unwrap();
        assert_eq!(hw.values[&0], 0);
        let mut hw = Fake::new();
        let s = service(&mut hw, BTreeMap::from([(1, 1)]), false);
        assert_eq!(hw.writes, vec![(0, 1)]);
        assert_eq!(s.actual.get(&0), Some(&0));
        assert_eq!(s.actual.get(&1), Some(&1));
    }
    #[test]
    fn failed_mutation_reports_actual_without_persisting_target() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        s.take_updates();
        hw.fail_set = true;
        assert!(s.request(2, 130, &mut hw, at(1)).is_err());
        assert_eq!(s.actual.get(&2), Some(&130));
        assert_eq!(s.confirmed(2), Some(128));
        assert!(!s.values.dirty());
        assert!(s.take_updates().contains(&("brightness", "130".into())));
    }
    #[test]
    fn pending_target_is_not_state_or_persistent_until_active_readback() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        hw.queued = true;
        s.request(10, 10, &mut hw, at(1)).unwrap();
        assert_eq!(s.actual.get(&10), Some(&20));
        assert_eq!(s.confirmed(10), Some(20));
        assert!(!s.values.dirty());
        assert!(s.request(10, 5, &mut hw, at(2)).is_err());
        hw.values.insert(20, 10);
        s.tick(&mut hw, at(2), false).unwrap();
        assert!(s.pending.is_empty());
        assert_eq!(s.confirmed(10), Some(10));
        assert!(!s.values.due(at(11)));
        assert!(s.values.due(at(12)));
    }
    #[test]
    fn fps_and_gop_pending_changes_exclude_each_other_per_channel() {
        let mut hw = Fake::new();
        hw.values.insert(21, 20);
        hw.values.insert(25, 1);
        let mut s = service(&mut hw, BTreeMap::new(), false);
        hw.queued = true;
        s.request(10, 10, &mut hw, at(1)).unwrap();
        assert!(s.request(14, 2, &mut hw, at(1)).is_err());
        // The other channel is independent, including the reverse conflict.
        s.request(15, 2, &mut hw, at(1)).unwrap();
        assert!(s.request(11, 10, &mut hw, at(1)).is_err());
        assert_eq!(hw.writes, vec![(20, 10), (25, 2)]);
        hw.values.insert(20, 10);
        hw.values.insert(24, 2); // real secondary readback, never a desired substitute
        s.tick(&mut hw, at(2), false).unwrap();
        assert_eq!(s.actual.get(&14), Some(&2));
        assert_eq!(s.confirmed(14), Some(1));
        s.request(14, 3, &mut hw, at(2)).unwrap();
        assert_eq!(hw.writes.last(), Some(&(24, 3)));
    }
    #[test]
    fn multiplier_pending_excludes_all_same_channel_graph_controls() {
        for first in [20, 24, 26] {
            let mut hw = Fake::new();
            hw.values.insert(21, 20);
            hw.values.insert(25, 1);
            let mut s = service(&mut hw, BTreeMap::new(), false);
            hw.queued = true;
            let setting = |control| settings::slot_of(control).unwrap();
            let target = |control| {
                if control <= 21 {
                    10
                } else if control <= 25 {
                    2
                } else {
                    1
                }
            };
            s.request(setting(first), target(first), &mut hw, at(1))
                .unwrap();
            for second in [20, 24, 26] {
                assert_eq!(
                    s.request(setting(second), target(second), &mut hw, at(1))
                        .unwrap_err()
                        .kind(),
                    io::ErrorKind::WouldBlock
                );
            }
            s.request(setting(first + 1), target(first + 1), &mut hw, at(1))
                .unwrap();
            assert_eq!(hw.writes.len(), 2);
        }
    }
    #[test]
    fn saved_coupled_controls_wait_for_confirmation_and_new_commands_supersede_them() {
        let mut hw = Fake::new();
        hw.queued = true;
        let mut s = service(&mut hw, BTreeMap::from([(10, 10), (14, 2), (16, 1)]), false);
        assert_eq!(hw.writes, vec![(20, 10)]);
        assert_eq!(s.startup, BTreeMap::from([(14, 2), (16, 1)]));
        hw.values.insert(20, 10);
        s.tick(&mut hw, at(1), false).unwrap();
        assert_eq!(hw.writes, vec![(20, 10), (24, 2)]);
        assert_eq!(s.startup, BTreeMap::from([(16, 1)]));
        hw.values.insert(24, 2);
        s.tick(&mut hw, at(2), false).unwrap();
        assert_eq!(hw.writes.last(), Some(&(26, 1)));
        hw.values.insert(26, 1);
        s.tick(&mut hw, at(3), false).unwrap();
        assert_eq!(s.confirmed(16), Some(1));
        assert!(s.startup.is_empty() && s.pending.is_empty());

        let mut hw = Fake::new();
        hw.queued = true;
        let mut s = service(&mut hw, BTreeMap::from([(10, 10), (16, 1)]), false);
        assert!(s.request(16, 2, &mut hw, at(1)).is_err());
        assert!(s.startup.is_empty());
        hw.values.insert(20, 10);
        s.tick(&mut hw, at(2), false).unwrap();
        assert_eq!(hw.writes, vec![(20, 10)]);
    }
    #[test]
    fn startup_publishes_saved_values_never_the_hardware_value_before_the_apply() {
        let key = |slot| settings::spec(slot).unwrap().key;
        let mut hw = Fake::new();
        hw.queued = true;
        let mut s = service(&mut hw, BTreeMap::from([(10, 15), (16, 1)]), false);
        let first = s.take_updates();
        // Main FPS 20 and multiplier 3 are what the hardware runs before the saved apply.
        for (slot, value) in [(10, "15"), (16, "1")] {
            assert!(first.contains(&(key(slot), value.into())), "{first:?}");
        }
        assert!(!first.contains(&(key(10), "20".into())));
        assert!(!first.contains(&(key(16), "3".into())));
        s.tick(&mut hw, at(1), false).unwrap();
        let held = s.take_updates();
        assert!(
            held.iter().all(|(k, _)| ![key(10), key(16)].contains(k)),
            "{held:?}"
        );
        hw.values.insert(20, 15);
        s.tick(&mut hw, at(2), false).unwrap();
        assert!(s.take_updates().contains(&(key(10), "15".into())));
        hw.values.insert(26, 1);
        s.tick(&mut hw, at(3), false).unwrap();
        assert!(s.take_updates().contains(&(key(16), "1".into())));
        assert!(s.held.is_empty());
        // A saved value the hardware never confirms is rolled back; then what is in force publishes.
        let mut hw = Fake::new();
        hw.queued = true;
        let mut s = service(&mut hw, BTreeMap::from([(10, 15)]), false);
        assert!(s.take_updates().contains(&(key(10), "15".into())));
        s.tick(&mut hw, at(6), false).unwrap();
        s.tick(&mut hw, at(6), false).unwrap();
        assert!(s.take_updates().contains(&(key(10), "20".into())));
        assert!(s.held.is_empty());
    }
    #[test]
    fn gop_refresh_marks_failed_readback_unknown_and_recovers_without_persisting() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        s.take_updates();
        hw.values.remove(&24);
        s.tick(&mut hw, at(1), false).unwrap();
        let key = settings::spec(14).unwrap().key;
        assert!(s.take_updates().contains(&(key, "unknown".into())));
        assert!(!s.actual.contains_key(&14));
        hw.values.insert(24, 2);
        s.tick(&mut hw, at(2), false).unwrap();
        assert!(s.take_updates().contains(&(key, "2".into())));
        assert_eq!(s.confirmed(14), Some(1));
        assert!(!s.values.dirty());
    }
    #[test]
    fn timeout_rolls_back_once_and_stops_after_a_second_bounded_window() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        hw.queued = true;
        s.request(14, 2, &mut hw, at(1)).unwrap();
        hw.values.insert(24, 3);
        s.tick(&mut hw, at(6), false).unwrap();
        assert_eq!(hw.writes, vec![(24, 2), (24, 1)]);
        s.tick(&mut hw, at(11), false).unwrap();
        assert!(s.pending.is_empty());
        s.tick(&mut hw, at(100), false).unwrap();
        assert_eq!(hw.writes.len(), 2);
        assert_eq!(s.actual.get(&14), Some(&3));
        assert_eq!(s.confirmed(14), Some(1));
        assert!(!s.values.dirty());
    }
    #[test]
    fn rollback_readback_can_finish_without_saving_failed_target() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        hw.queued = true;
        s.request(10, 10, &mut hw, at(1)).unwrap();
        s.tick(&mut hw, at(6), false).unwrap();
        s.tick(&mut hw, at(6), false).unwrap();
        assert!(s.pending.is_empty() && !s.values.dirty());
        assert_eq!(s.confirmed(10), Some(20));
    }
    #[test]
    fn read_failures_unverified_and_invalid_requests_never_confirm() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        assert!(s.request(2, 256, &mut hw, at(1)).is_err());
        assert!(hw.writes.is_empty());
        hw.fail_get = true;
        assert!(s.request(2, 130, &mut hw, at(1)).is_err());
        assert!(hw.writes.is_empty());
        hw.fail_get = false;
        hw.unverified = true;
        assert!(s.request(2, 130, &mut hw, at(1)).is_err());
        assert_eq!(s.confirmed(2), Some(128));
        assert!(!s.values.dirty());
    }
    #[test]
    fn late_target_and_failed_rollback_never_confirm_or_retry() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        hw.queued = true;
        s.request(10, 10, &mut hw, at(1)).unwrap();
        hw.values.insert(20, 10); // first observed after the five-second budget
        hw.fail_set = true;
        s.tick(&mut hw, at(7), false).unwrap();
        assert!(s.pending.is_empty());
        assert_eq!(hw.writes, vec![(20, 10), (20, 20)]);
        assert_eq!(s.actual.get(&10), Some(&10));
        assert_eq!(s.confirmed(10), Some(20));
        assert!(!s.values.dirty());
        s.tick(&mut hw, at(100), false).unwrap();
        assert_eq!(hw.writes.len(), 2);
    }
    #[test]
    fn failed_pending_reads_still_have_a_bounded_rollback_lifecycle() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        hw.queued = true;
        s.request(10, 10, &mut hw, at(1)).unwrap();
        hw.fail_get = true;
        s.tick(&mut hw, at(6), false).unwrap();
        s.tick(&mut hw, at(11), false).unwrap();
        assert!(s.pending.is_empty());
        assert_eq!(hw.writes, vec![(20, 10), (20, 20)]);
        assert!(!s.values.dirty());
    }
    #[test]
    fn command_bursts_coalesce_updates_by_catalog_key() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        s.take_updates();
        for value in 0..=255 {
            s.request(2, value, &mut hw, at(1)).unwrap();
        }
        assert_eq!(s.updates.len(), 2);
        let updates = s.take_updates();
        assert!(updates.contains(&("brightness", "255".into())));
        assert!(s.take_updates().is_empty());
    }
    #[test]
    fn save_failure_preserves_first_dirty_deadline_and_unhealthy_defers_io() {
        let mut hw = Fake::new();
        let mut s = service(&mut hw, BTreeMap::new(), false);
        s.request(2, 129, &mut hw, at(1)).unwrap();
        s.request(7, 400, &mut hw, at(9)).unwrap();
        assert!(!s.values.due(at(10)));
        assert!(s.values.due(at(11)));
        s.tick(&mut hw, at(11), false).unwrap();
        assert!(s.values.dirty());
        s.flush().unwrap();
        assert!(s.values.dirty()); // probation never writes
        assert!(s.tick(&mut hw, at(11), true).is_err());
        // Changed deliberately: the shared store retries a failed write one save window later.
        assert!(!s.values.due(at(20)) && s.values.due(at(21)));
        assert!(s.values.dirty());
        assert_eq!(s.confirmed(7), Some(400));
    }
}
