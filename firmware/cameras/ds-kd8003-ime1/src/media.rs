//! The one owner of the gate's live settings on the media side: it applies a changed value to the
//! codec, the speaker, an encoder channel or the ISP, reads back what is in force, keeps
//! [`Values`] (persistence, the ONVIF read API and change feed) and says which states to publish.
//! The hardware sits behind [`Media`]; the decisions here are host-tested with a fake.
use crate::settings::{self, ISP_FORCED, ISP_SATURATION, MAIN_SIZES, PROFILES, Target};
use camera_transport::settings::{Announcer, Codec, Disk, Values};
use std::time::Duration;

/// Encoder parameters of one channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream {
    pub w: u32,
    pub h: u32,
    pub fps: u32,
    pub kbps: u32,
    pub gop: u32,
    pub vbr: bool,
    /// Index into [`PROFILES`], the bridge's numbering.
    pub profile: u32,
}

/// How a stream change reaches the encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// `HI_MPI_VENC_SetChnAttr` and an IDR request: rate, frame rate, GOP, rate control.
    Attr,
    /// Destroy and create the VENC channel: a profile or size change on the same VPSS channel.
    Recreate,
    /// Also resize the VPSS channel that feeds it; announced to the change feed's subscribers.
    Rebuild,
}

/// Where the main stream comes from for a size, as the bridge wires it.
const FULL: (u32, u32) = (1920, 1080);

/// The cheapest change that turns `old` into `new` on channel `chn`, given the size VPSS channel 2
/// (the scaled main stream) holds now.
pub fn change(
    chn: usize,
    old: &Stream,
    new: &Stream,
    scaled: Option<(u32, u32)>,
) -> Option<Change> {
    if old == new {
        return None;
    }
    let size = (new.w, new.h);
    if chn == 0 && (old.w, old.h) != size {
        return Some(if size != FULL && scaled != Some(size) {
            Change::Rebuild
        } else {
            Change::Recreate
        });
    }
    Some(if old.profile != new.profile || (old.w, old.h) != size {
        Change::Recreate
    } else {
        Change::Attr
    })
}

/// The board's media calls. Return codes are the bridge's (0 ok).
pub trait Media {
    /// acodec output (true) or input level, dB.
    fn codec_set(&mut self, output: bool, db: i32) -> i32;
    /// `(input, output)` levels in force.
    fn codec_get(&mut self) -> Option<(i32, i32)>;
    /// Restarts the speaker with these clip and talk gains.
    fn speaker(&mut self, clip_db: i32, talk_db: i32) -> i32;
    fn stream_set(&mut self, chn: usize, s: &Stream, how: Change) -> i32;
    fn stream_get(&mut self, chn: usize) -> Option<Stream>;
    fn isp_set(&mut self, key: u32, raw: u32) -> i32;
    fn isp_get(&mut self, key: u32) -> Option<u32>;
}

/// A state to publish: `(entity id, payload)`.
pub type State = (&'static str, String);

/// The apply owner of a static spec list. One thread holds it; others read through [`Values::reader`].
pub struct Owner<D: Disk, C: Codec> {
    values: Values<'static, D, C>,
    announcer: Announcer,
    /// Size VPSS channel 2 holds, once the bridge resized it.
    scaled: Option<(u32, u32)>,
    night: bool,
    log: Vec<String>,
}

impl<D: Disk, C: Codec> Owner<D, C> {
    pub fn new(values: Values<'static, D, C>) -> Self {
        let announcer = values.announcer();
        Self {
            values,
            announcer,
            scaled: None,
            night: false,
            log: Vec::new(),
        }
    }

    pub fn values(&self) -> &Values<'static, D, C> {
        &self.values
    }

    /// Lines for the device log since the last call.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    fn get(&self, slot: u16) -> i32 {
        self.values.get(slot).unwrap_or(0)
    }

    fn key(&self, key: &str) -> i32 {
        self.values.value(key).unwrap_or(0)
    }

    /// The stream the values describe.
    pub fn stream(&self, chn: usize) -> Stream {
        let p = if chn == 0 { "main" } else { "sub" };
        let at = |k: &str| self.key(&format!("{p}_{k}"));
        let (w, h) = if chn == 0 {
            parse_size(MAIN_SIZES[at("resolution") as usize]).unwrap_or(FULL)
        } else {
            settings::SUB_SIZE
        };
        Stream {
            w,
            h,
            fps: at("fps") as u32,
            kbps: at("kbps") as u32,
            gop: at("gop") as u32,
            vbr: at("rate_control") == 1,
            profile: at("profile") as u32,
        }
    }

    /// Puts what the hardware holds into the values; returns the states of the specs given.
    fn settle(&mut self, slot: u16, value: i32, now: Duration) {
        if let Err(e) = self.values.put(slot, value, now) {
            self.log.push(format!("settings: {e}"));
        }
    }

    fn states_of(&self, slots: &[u16]) -> Vec<State> {
        slots
            .iter()
            .filter_map(|&slot| {
                let spec = self.values.spec(slot)?;
                Some((spec.key, spec.render(self.get(slot))))
            })
            .collect()
    }

    /// Every state, for the first publication.
    pub fn states(&self) -> Vec<State> {
        self.values.states().collect()
    }

    /// Applies what the values hold at start, after media came up. `seed_isp` (no settings file
    /// existed) adopts what the ISP reports instead of pushing defaults.
    pub fn boot<M: Media>(&mut self, m: &mut M, now: Duration) {
        let seed_isp = self.values.first_run();
        for output in [true, false] {
            let slot = if output {
                settings::CODEC_OUT_DB
            } else {
                settings::MIC_GAIN_DB
            };
            let rc = m.codec_set(output, self.get(slot));
            if rc != 0 {
                self.log.push(format!(
                    "codec: {} rc {rc}",
                    if output { "output" } else { "input" }
                ));
            }
        }
        // The main encoder starts with the bridge's profile; the sub channel is created here.
        let main = self.stream(0);
        match m.stream_get(0) {
            Some(live) => {
                if let Some(how) = change(0, &live, &main, self.scaled) {
                    self.apply_stream(m, 0, live, main, how, now);
                }
            }
            None => {
                // The bridge does not report the channel: create it from the values.
                let how = if (main.w, main.h) == FULL {
                    Change::Recreate
                } else {
                    Change::Rebuild
                };
                self.apply_stream(m, 0, main, main, how, now);
            }
        }
        let sub = self.stream(1);
        let rc = m.stream_set(1, &sub, Change::Recreate);
        self.log.push(format!("stream sub: start rc {rc}"));
        self.read_stream(m, 1, sub, now);
        let specs = self.values.specs();
        for spec in specs {
            let Some(Target::Isp(key)) = settings::target(spec.slot) else {
                continue;
            };
            let stored = self.get(spec.slot);
            let read = m
                .isp_get(key)
                .and_then(|raw| settings::isp_value(key, raw))
                .filter(|v| spec.valid(*v));
            let forced = ISP_FORCED.contains(&key);
            if seed_isp && !forced {
                if let Some(v) = read {
                    self.settle(spec.slot, v, now);
                }
                continue;
            }
            if isp_boot_applies(forced, read, stored, spec.default) {
                self.apply_isp(m, spec.slot, key, stored, now);
            }
        }
    }

    /// The day/night decision: monochrome by night, the saturation setting by day.
    pub fn night<M: Media>(&mut self, m: &mut M, night: bool) {
        self.night = night;
        let level = if night {
            0
        } else {
            self.get(settings::ISP_SLOT + ISP_SATURATION as u16)
        };
        let rc = m.isp_set(ISP_SATURATION, level as u32);
        self.log.push(format!(
            "image: {} rc {rc}",
            if night { "monochrome" } else { "colour" }
        ));
    }

    /// One `Command::Setting` from MQTT. Returns the states to publish: the values in force of the
    /// settings it touched, also when the apply failed or the value was rejected.
    pub fn set<M: Media>(&mut self, m: &mut M, slot: u16, value: i32, now: Duration) -> Vec<State> {
        let Some(spec) = self.values.spec(slot) else {
            self.log.push(format!("settings: unknown slot {slot}"));
            return Vec::new();
        };
        if !spec.valid(value) {
            self.log.push(format!(
                "settings: {} = {value} rejected: out of range",
                spec.key
            ));
            return self.states_of(&[slot]);
        }
        if self.values.get(slot) == Some(value) {
            return self.states_of(&[slot]);
        }
        match settings::target(slot) {
            Some(Target::Door) | None => self.settle(slot, value, now),
            Some(Target::Codec { output }) => {
                let rc = m.codec_set(output, value);
                let read = m.codec_get().map(|(i, o)| if output { o } else { i });
                let in_force = match read.filter(|v| spec.valid(*v)) {
                    Some(v) => v,
                    None if rc == 0 => value,
                    None => self.get(slot),
                };
                if rc != 0 || in_force != value {
                    self.log.push(format!(
                        "codec: {} = {value} rc {rc}, in force {in_force}",
                        spec.key
                    ));
                }
                self.settle(slot, in_force, now);
            }
            Some(Target::Speaker) => {
                let (mut clip, mut talk) =
                    (self.get(settings::CLIP_DB), self.get(settings::TALK_DB));
                if slot == settings::CLIP_DB {
                    clip = value;
                } else {
                    talk = value;
                }
                let rc = m.speaker(clip, talk);
                if rc == 0 {
                    self.settle(slot, value, now);
                } else {
                    self.log
                        .push(format!("speaker: {} = {value} rc {rc}", spec.key));
                }
            }
            Some(Target::Stream(chn)) => {
                let old = self.stream(chn);
                self.settle(slot, value, now);
                let new = self.stream(chn);
                return match change(chn, &old, &new, self.scaled) {
                    Some(how) => self.apply_stream(m, chn, old, new, how, now),
                    None => self.states_of(&[slot]),
                };
            }
            Some(Target::Isp(key)) => {
                if key == ISP_SATURATION && self.night {
                    // Monochrome stays in force until day; the setting applies then.
                    self.settle(slot, value, now);
                } else {
                    self.apply_isp(m, slot, key, value, now);
                }
            }
        }
        self.states_of(&[slot])
    }

    fn apply_isp<M: Media>(&mut self, m: &mut M, slot: u16, key: u32, value: i32, now: Duration) {
        let Some(spec) = self.values.spec(slot) else {
            return;
        };
        let rc = m.isp_set(key, settings::isp_raw(key, value));
        let read = m
            .isp_get(key)
            .and_then(|raw| settings::isp_value(key, raw))
            .filter(|v| spec.valid(*v));
        let in_force = match (rc, read) {
            (_, Some(v)) => v,
            (0, None) => value,
            _ => self.get(slot),
        };
        self.log.push(format!(
            "image: {} (key {key}) = {value} rc {rc}, reads {read:?}",
            spec.key
        ));
        self.settle(slot, in_force, now);
    }

    fn apply_stream<M: Media>(
        &mut self,
        m: &mut M,
        chn: usize,
        old: Stream,
        new: Stream,
        how: Change,
        now: Duration,
    ) -> Vec<State> {
        if how == Change::Rebuild {
            self.announcer.rebuilding();
        }
        let mut rc = m.stream_set(chn, &new, how);
        if rc != 0 && how == Change::Attr {
            // Not every attribute may change live on this encoder: recreate the channel instead.
            self.log
                .push(format!("stream {chn}: live change rc {rc}, recreating"));
            rc = m.stream_set(chn, &new, Change::Recreate);
        }
        if how == Change::Rebuild {
            if rc == 0 {
                self.scaled = Some((new.w, new.h));
            }
            self.announcer.rebuilt();
        }
        self.log.push(format!(
            "stream {chn}: {old:?} -> {new:?} by {how:?} rc {rc}"
        ));
        let fallback = if rc == 0 { new } else { old };
        self.read_stream(m, chn, fallback, now)
    }

    /// Puts the encoder's read-back parameters (or `fallback`) into the values.
    fn read_stream<M: Media>(
        &mut self,
        m: &mut M,
        chn: usize,
        fallback: Stream,
        now: Duration,
    ) -> Vec<State> {
        let s = m.stream_get(chn).unwrap_or(fallback);
        let p = if chn == 0 { "main" } else { "sub" };
        let mut slots = Vec::new();
        let mut put = |owner: &mut Self, k: &str, v: Option<i32>| {
            let Some(spec) = owner.values.spec_by_key(&format!("{p}_{k}")) else {
                return;
            };
            slots.push(spec.slot);
            if let Some(v) = v.filter(|v| spec.valid(*v)) {
                owner.settle(spec.slot, v, now);
            }
        };
        let size = format!("{}x{}", s.w, s.h);
        put(
            self,
            "resolution",
            MAIN_SIZES.iter().position(|o| *o == size).map(|i| i as i32),
        );
        put(self, "fps", i32::try_from(s.fps).ok());
        put(self, "kbps", i32::try_from(s.kbps).ok());
        put(self, "gop", i32::try_from(s.gop).ok());
        put(self, "rate_control", Some(i32::from(s.vbr)));
        put(
            self,
            "profile",
            (s.profile < PROFILES.len() as u32).then_some(s.profile as i32),
        );
        self.states_of(&slots)
    }

    /// Saves within the window; call often.
    pub fn tick(&mut self, now: Duration) {
        if let Err(e) = self.values.tick(now) {
            self.log.push(format!("settings: save failed: {e}"));
        }
    }

    pub fn flush(&mut self) {
        if let Err(e) = self.values.flush() {
            self.log.push(format!("settings: save failed: {e}"));
        }
    }
}

/// Whether start pushes a stored ISP value: always for forced keys; otherwise only when the ISP
/// reads something else, or, for a key that cannot be read, when the owner changed it from the
/// default (a default is never pushed blind).
pub fn isp_boot_applies(forced: bool, read: Option<i32>, stored: i32, default: i32) -> bool {
    forced
        || match read {
            Some(r) => r != stored,
            None => stored != default,
        }
}

fn parse_size(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SPECS;
    use camera_transport::settings::{Event, JsonCodec};
    use std::collections::BTreeMap;
    use std::io;

    #[derive(Default)]
    struct Mem(Option<Vec<u8>>);
    impl Disk for Mem {
        fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
            Ok(self.0.clone())
        }
        fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.0 = Some(bytes.to_vec());
            Ok(())
        }
    }

    /// The board as the bridge behaves: keys it rejects, levels it clamps, live attribute changes
    /// it refuses.
    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        codec: (i32, i32),
        clamp_output: Option<i32>,
        streams: [Option<Stream>; 2],
        attr_fails: bool,
        isp: BTreeMap<u32, u32>,
        rejects: Vec<u32>,
        unreadable: Vec<u32>,
    }

    impl Media for Fake {
        fn codec_set(&mut self, output: bool, db: i32) -> i32 {
            self.calls.push(format!("codec {output} {db}"));
            if output {
                self.codec.1 = self.clamp_output.map_or(db, |c| db.min(c));
            } else {
                self.codec.0 = db;
            }
            0
        }
        fn codec_get(&mut self) -> Option<(i32, i32)> {
            Some(self.codec)
        }
        fn speaker(&mut self, clip: i32, talk: i32) -> i32 {
            self.calls.push(format!("speaker {clip} {talk}"));
            0
        }
        fn stream_set(&mut self, chn: usize, s: &Stream, how: Change) -> i32 {
            self.calls.push(format!("stream {chn} {how:?}"));
            if how == Change::Attr && self.attr_fails {
                return -1;
            }
            self.streams[chn] = Some(*s);
            0
        }
        fn stream_get(&mut self, chn: usize) -> Option<Stream> {
            self.streams[chn]
        }
        fn isp_set(&mut self, key: u32, raw: u32) -> i32 {
            self.calls.push(format!("isp {key} {raw}"));
            if self.rejects.contains(&key) {
                return 0x4000_0004;
            }
            self.isp.insert(key, raw);
            0
        }
        fn isp_get(&mut self, key: u32) -> Option<u32> {
            if self.unreadable.contains(&key) {
                return None;
            }
            self.isp.get(&key).copied()
        }
    }

    fn main_stream() -> Stream {
        Stream {
            w: 1920,
            h: 1080,
            fps: 30,
            kbps: 4096,
            gop: 30,
            vbr: true,
            profile: 0,
        }
    }

    fn owner(file: Option<&str>) -> Owner<Mem, JsonCodec> {
        let (values, problems) =
            Values::load(SPECS, Mem(file.map(|f| f.as_bytes().to_vec())), JsonCodec);
        assert!(problems.is_empty(), "{problems:?}");
        Owner::new(values)
    }

    fn slot(key: &str) -> u16 {
        SPECS.iter().find(|s| s.key == key).unwrap().slot
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn stream_changes_take_the_cheapest_path() {
        let old = main_stream();
        let rate = Stream {
            kbps: 2048,
            fps: 15,
            gop: 60,
            vbr: false,
            ..old
        };
        assert_eq!(change(0, &old, &old, None), None);
        assert_eq!(change(0, &old, &rate, None), Some(Change::Attr));
        assert_eq!(
            change(0, &old, &Stream { profile: 2, ..old }, None),
            Some(Change::Recreate)
        );
        let hd = Stream {
            w: 1280,
            h: 720,
            ..old
        };
        assert_eq!(
            change(0, &old, &hd, None),
            Some(Change::Rebuild),
            "VPSS channel 2 needs the size"
        );
        assert_eq!(
            change(0, &old, &hd, Some((1280, 720))),
            Some(Change::Recreate),
            "already that size"
        );
        assert_eq!(
            change(0, &hd, &old, Some((1280, 720))),
            Some(Change::Recreate),
            "back to channel 0"
        );
        let sub = Stream {
            w: 704,
            h: 480,
            ..old
        };
        assert_eq!(
            change(1, &sub, &Stream { fps: 10, ..sub }, None),
            Some(Change::Attr)
        );
    }

    #[test]
    fn a_rate_change_is_live_and_published_as_read_back() {
        let mut o = owner(None);
        let mut m = Fake {
            streams: [Some(main_stream()), None],
            ..Fake::default()
        };
        let states = o.set(&mut m, slot("main_kbps"), 2048, secs(1));
        assert_eq!(m.calls, ["stream 0 Attr"]);
        assert!(states.contains(&("main_kbps", "2048".into())));
        assert_eq!(o.values().value("main_kbps"), Some(2048));
        assert!(o.values().dirty());
    }

    #[test]
    fn a_refused_live_change_recreates_the_channel() {
        let mut o = owner(None);
        let mut m = Fake {
            streams: [Some(main_stream()), None],
            attr_fails: true,
            ..Fake::default()
        };
        o.set(&mut m, slot("main_rate_control"), 0, secs(1));
        assert_eq!(m.calls, ["stream 0 Attr", "stream 0 Recreate"]);
        assert!(!m.streams[0].unwrap().vbr);
    }

    #[test]
    fn a_vpss_resize_is_announced_around_the_rebuild_and_only_once() {
        let mut o = owner(None);
        let feed = o.values().reader().subscribe();
        let mut m = Fake {
            streams: [Some(main_stream()), None],
            ..Fake::default()
        };
        let states = o.set(&mut m, slot("main_resolution"), 1, secs(1));
        assert_eq!(m.calls, ["stream 0 Rebuild"]);
        assert!(states.contains(&("main_resolution", "1280x720".into())));
        let events: Vec<Event> = feed.try_iter().collect();
        assert_eq!(events.first(), Some(&Event::Changed { slot: 20, value: 1 }));
        assert!(events.contains(&Event::PipelineRebuilding));
        assert_eq!(events.last(), Some(&Event::PipelineRebuilt));
        o.set(&mut m, slot("main_resolution"), 0, secs(2));
        o.set(&mut m, slot("main_resolution"), 1, secs(3));
        assert_eq!(m.calls[1..], ["stream 0 Recreate", "stream 0 Recreate"]);
    }

    #[test]
    fn a_failed_stream_change_publishes_what_the_encoder_kept() {
        let mut o = owner(None);
        struct Broken(Fake);
        impl Media for Broken {
            fn codec_set(&mut self, o: bool, d: i32) -> i32 {
                self.0.codec_set(o, d)
            }
            fn codec_get(&mut self) -> Option<(i32, i32)> {
                self.0.codec_get()
            }
            fn speaker(&mut self, c: i32, t: i32) -> i32 {
                self.0.speaker(c, t)
            }
            fn stream_set(&mut self, _: usize, _: &Stream, _: Change) -> i32 {
                -110
            }
            fn stream_get(&mut self, c: usize) -> Option<Stream> {
                self.0.stream_get(c)
            }
            fn isp_set(&mut self, k: u32, r: u32) -> i32 {
                self.0.isp_set(k, r)
            }
            fn isp_get(&mut self, k: u32) -> Option<u32> {
                self.0.isp_get(k)
            }
        }
        let mut m = Broken(Fake {
            streams: [Some(main_stream()), None],
            ..Fake::default()
        });
        let states = o.set(&mut m, slot("main_profile"), 2, secs(1));
        assert!(states.contains(&("main_profile", "baseline".into())));
        assert_eq!(o.values().value("main_profile"), Some(0));
    }

    #[test]
    fn a_rejected_isp_key_publishes_the_value_in_force() {
        let mut o = owner(None);
        let mut m = Fake {
            rejects: vec![39],
            ..Fake::default()
        };
        m.isp.insert(39, 100);
        let states = o.set(&mut m, slot("isp_gain_limit"), 40, secs(1));
        assert_eq!(states, [("isp_gain_limit", "100".into())]);
        assert!(!o.values().dirty(), "nothing changed");
        let states = o.set(&mut m, slot("isp_brightness"), 70, secs(1));
        assert_eq!(states, [("isp_brightness", "70".into())]);
        assert_eq!(m.isp[&33], 70);
    }

    #[test]
    fn out_of_range_and_equal_values_touch_nothing_but_republish() {
        let mut o = owner(None);
        let mut m = Fake::default();
        assert_eq!(
            o.set(&mut m, slot("isp_brightness"), 101, secs(1)),
            [("isp_brightness", "50".into())]
        );
        assert_eq!(
            o.set(&mut m, slot("open_seconds"), 5, secs(1)),
            [("open_seconds", "5".into())]
        );
        assert!(m.calls.is_empty());
        o.set(&mut m, slot("open_seconds"), 12, secs(1));
        assert_eq!(o.values().value("open_seconds"), Some(12));
        assert!(
            m.calls.is_empty(),
            "the door reads its pulse where it is used"
        );
    }

    #[test]
    fn codec_levels_publish_what_the_codec_reads_back() {
        let mut o = owner(None);
        let mut m = Fake {
            clamp_output: Some(3),
            ..Fake::default()
        };
        assert_eq!(
            o.set(&mut m, slot("codec_out_db"), 6, secs(1)),
            [("codec_out_db", "3".into())]
        );
        assert_eq!(
            o.set(&mut m, slot("mic_gain_db"), 20, secs(1)),
            [("mic_gain_db", "20".into())]
        );
        assert_eq!(m.codec, (20, 3));
    }

    #[test]
    fn speaker_gains_restart_the_speaker_with_both_values() {
        let mut o = owner(None);
        let mut m = Fake::default();
        o.set(&mut m, slot("talk_db"), 12, secs(1));
        o.set(&mut m, slot("clip_db"), -6, secs(1));
        assert_eq!(m.calls, ["speaker 0 12", "speaker -6 12"]);
    }

    #[test]
    fn saturation_waits_for_day_and_night_is_monochrome() {
        let mut o = owner(None);
        let mut m = Fake::default();
        o.night(&mut m, true);
        assert_eq!(
            o.set(&mut m, slot("isp_saturation"), 70, secs(1)),
            [("isp_saturation", "70".into())]
        );
        assert_eq!(m.calls, ["isp 84 0"]);
        o.night(&mut m, false);
        assert_eq!(m.calls, ["isp 84 0", "isp 84 70"]);
    }

    #[test]
    fn first_boot_adopts_the_isp_and_forces_the_awb_mode() {
        let mut o = owner(None);
        let mut m = Fake {
            streams: [Some(main_stream()), None],
            ..Fake::default()
        };
        m.isp.insert(33, 55);
        m.isp.insert(82, 0);
        m.isp.insert(145, 0);
        o.boot(&mut m, secs(0));
        assert_eq!(o.values().value("isp_brightness"), Some(55));
        assert_eq!(
            o.values().value("isp_sharpness"),
            Some(0),
            "the ISP's value read back"
        );
        assert_eq!(m.isp[&82], 1, "AWB auto against the magenta image");
        assert!(m.calls.contains(&"stream 1 Recreate".to_string()));
        assert!(
            !m.calls.iter().any(|c| c.starts_with("stream 0")),
            "main runs as started"
        );
        assert!(!m.calls.iter().any(|c| c.starts_with("isp 33")));
    }

    #[test]
    fn a_later_boot_applies_stored_values_that_differ() {
        let file =
            r#"{"version":1,"values":{"isp_brightness":70,"isp_gain_limit":60,"main_fps":15}}"#;
        let mut o = owner(Some(file));
        let mut m = Fake {
            streams: [Some(main_stream()), None],
            unreadable: vec![39],
            ..Fake::default()
        };
        m.isp.insert(33, 50);
        m.isp.insert(144, 50);
        o.boot(&mut m, secs(0));
        assert!(m.calls.contains(&"isp 33 70".to_string()));
        assert!(
            m.calls.contains(&"isp 39 60".to_string()),
            "unreadable but changed by the owner"
        );
        assert!(
            !m.calls.iter().any(|c| c.starts_with("isp 144")),
            "already in force"
        );
        assert!(
            !m.calls.iter().any(|c| c.starts_with("isp 148")),
            "unreadable default: never pushed"
        );
        assert!(m.calls.contains(&"stream 0 Attr".to_string()));
        assert_eq!(m.streams[0].unwrap().fps, 15);
    }

    #[test]
    fn boot_isp_rule() {
        assert!(isp_boot_applies(true, Some(1), 1, 1));
        assert!(!isp_boot_applies(false, Some(5), 5, 0));
        assert!(isp_boot_applies(false, Some(5), 6, 6));
        assert!(!isp_boot_applies(false, None, 6, 6));
        assert!(isp_boot_applies(false, None, 7, 6));
    }
}
