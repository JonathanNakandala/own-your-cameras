//! Live settings shared by every camera: the spec list, the values in force, their persistence, and
//! a read API with a change feed for other owners (ONVIF).
//!
//! Each camera keeps one `&[Spec]` list, the only list of its keys: payload parsing, validation, the
//! file format and the state texts derive from it. Every value is an `i32`: a number, a switch as
//! 0/1, a select as the option index, matching `mqtt::Command::Setting`. Loading never fails: a bad
//! file or field falls back to the default and is reported as a [`Problem`]. A change is on disk
//! within [`SAVE_WITHIN`] of the first unsaved change, and a burst of changes costs one write.
//! Camera-specific apply logic (probes, rollback, cross-field rules) stays in the camera, on top of
//! [`Values`].
//!
//! [`Spec::entity`] is the Home Assistant entity of a spec (`entity_category: config`); the MQTT
//! layer validates its payloads and yields `Command::Setting { slot, value }`.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// A change is saved at most this long after the first unsaved change; writes are at least this far
/// apart, and a failed write is retried one window later.
pub const SAVE_WITHIN: Duration = Duration::from_secs(10);
/// Largest settings file read or written.
pub const MAX_FILE: usize = 16 * 1024;

/// What a setting is in Home Assistant, and how its `i32` value reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Number {
        min: i32,
        max: i32,
        unit: Option<&'static str>,
        /// A slider in HA; a box otherwise.
        slider: bool,
    },
    /// 0 = OFF, 1 = ON.
    Switch,
    /// The value is an index into the options.
    Select(&'static [&'static str]),
}

/// One setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spec {
    /// The `Command::Setting` slot; unique within a camera.
    pub slot: u16,
    /// File key, MQTT entity id; unique within a camera.
    pub key: &'static str,
    pub name: &'static str,
    pub kind: Kind,
    /// First-run value.
    pub default: i32,
    pub icon: Option<&'static str>,
    /// The HA section device slug this setting sits on; `None` is the camera device.
    pub section: Option<&'static str>,
}

impl Spec {
    /// A slider number without icon or section.
    pub const fn number(
        slot: u16,
        key: &'static str,
        name: &'static str,
        (min, max): (i32, i32),
        unit: Option<&'static str>,
        default: i32,
    ) -> Self {
        Self::new(
            slot,
            key,
            name,
            Kind::Number {
                min,
                max,
                unit,
                slider: true,
            },
            default,
        )
    }

    pub const fn switch(slot: u16, key: &'static str, name: &'static str, on: bool) -> Self {
        Self::new(slot, key, name, Kind::Switch, on as i32)
    }

    /// `default` is an option index.
    pub const fn select(
        slot: u16,
        key: &'static str,
        name: &'static str,
        options: &'static [&'static str],
        default: i32,
    ) -> Self {
        Self::new(slot, key, name, Kind::Select(options), default)
    }

    const fn new(
        slot: u16,
        key: &'static str,
        name: &'static str,
        kind: Kind,
        default: i32,
    ) -> Self {
        Self {
            slot,
            key,
            name,
            kind,
            default,
            icon: None,
            section: None,
        }
    }

    pub const fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub const fn section(mut self, slug: &'static str) -> Self {
        self.section = Some(slug);
        self
    }

    /// The Home Assistant entity: `entity_category: config`, commands routed to this spec's slot.
    /// The section is not applied here; a camera that shows sections prefixes the name itself.
    pub fn entity(&self) -> crate::mqtt::Entity {
        use crate::mqtt::{Category, Domain, Entity, Mode};
        let domain = match self.kind {
            Kind::Number {
                min, max, slider, ..
            } => Domain::number(min, max, if slider { Mode::Slider } else { Mode::Box }),
            Kind::Switch => Domain::Switch,
            Kind::Select(options) => Domain::select(options.iter().copied()),
        };
        let mut entity = Entity::new(self.key, self.name, domain)
            .category(Category::Config)
            .slot(self.slot);
        if let Kind::Number {
            unit: Some(unit), ..
        } = self.kind
        {
            entity = entity.unit(unit);
        }
        if let Some(icon) = self.icon {
            entity = entity.icon(icon);
        }
        entity
    }

    pub fn valid(&self, value: i32) -> bool {
        match self.kind {
            Kind::Number { min, max, .. } => (min..=max).contains(&value),
            Kind::Switch => value == 0 || value == 1,
            Kind::Select(options) => usize::try_from(value).is_ok_and(|i| i < options.len()),
        }
    }

    /// Parses an MQTT payload: a whole number (HA may send `12.0`), `ON`/`OFF`, or an option.
    pub fn parse(&self, text: &str) -> Result<i32, Rejected> {
        let text = text.trim();
        let value = match self.kind {
            Kind::Number { .. } => {
                let n: f64 = text.parse().map_err(|_| self.rejected("not a number"))?;
                if !n.is_finite() || n.fract() != 0.0 || n < i32::MIN as f64 || n > i32::MAX as f64
                {
                    return Err(self.rejected("not a whole number"));
                }
                n as i32
            }
            Kind::Switch => match text {
                "ON" => 1,
                "OFF" => 0,
                _ => return Err(self.rejected("expected ON or OFF")),
            },
            Kind::Select(options) => options
                .iter()
                .position(|o| *o == text)
                .and_then(|i| i32::try_from(i).ok())
                .ok_or_else(|| self.rejected("unknown option"))?,
        };
        self.checked(value)
    }

    /// The MQTT state text of `value`; an invalid value renders empty.
    pub fn render(&self, value: i32) -> String {
        if !self.valid(value) {
            return String::new();
        }
        match self.kind {
            Kind::Number { .. } => value.to_string(),
            Kind::Switch => if value == 1 { "ON" } else { "OFF" }.into(),
            Kind::Select(options) => options[value as usize].into(),
        }
    }

    /// A file field: any kind accepts its `i32` form; a switch also a boolean, a select its option.
    fn read(&self, field: &Field) -> Option<i32> {
        let value = match (self.kind, field) {
            (_, Field::Int(n)) => i32::try_from(*n).ok()?,
            (Kind::Switch, Field::Bool(b)) => i32::from(*b),
            (Kind::Select(options), Field::Text(t)) => {
                i32::try_from(options.iter().position(|o| o == t)?).ok()?
            }
            _ => return None,
        };
        self.valid(value).then_some(value)
    }

    fn checked(&self, value: i32) -> Result<i32, Rejected> {
        if self.valid(value) {
            Ok(value)
        } else {
            Err(self.rejected("out of range"))
        }
    }

    fn rejected(&self, why: &str) -> Rejected {
        Rejected(format!("{}: {why}", self.key))
    }
}

/// Checks a camera's spec list: unique slots and keys, a non-empty range or option list, valid
/// defaults. Each camera calls it from a unit test.
pub fn check_catalog(specs: &[Spec]) -> Result<(), String> {
    for (i, s) in specs.iter().enumerate() {
        if let Some(other) = specs[..i]
            .iter()
            .find(|o| o.slot == s.slot || o.key == s.key)
        {
            return Err(format!("{} and {} share a slot or key", other.key, s.key));
        }
        match s.kind {
            Kind::Number { min, max, .. } if min > max => {
                return Err(format!("{}: empty range", s.key));
            }
            Kind::Select([]) => return Err(format!("{}: no options", s.key)),
            _ => {}
        }
        if !s.valid(s.default) {
            return Err(format!("{}: invalid default", s.key));
        }
    }
    Ok(())
}

/// A rejected value; the settings are unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected(pub String);

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Rejected {}

/// A setting (or the whole file, `key: None`) that fell back to its default, or a failed save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub key: Option<String>,
    pub reason: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.key {
            Some(k) => write!(f, "{k}: {}", self.reason),
            None => f.write_str(&self.reason),
        }
    }
}

const SAVE_FAILED: &str = "save failed";

/// One value as a file format holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    Int(i64),
    Bool(bool),
    Text(String),
    /// Anything else (a float, a table): always invalid.
    Other,
}

/// A settings file format. Errors never quote the file's bytes.
pub trait Codec {
    /// The fields of a document; unknown keys are kept and ignored by the caller.
    fn decode(&self, bytes: &[u8]) -> Result<BTreeMap<String, Field>, String>;
    /// A document holding every `(spec, value)`.
    fn encode(&self, values: &[(&Spec, i32)]) -> Result<Vec<u8>, String>;
}

/// Mi 360's format: `{"version":1,"values":{"key":<i32>,…}}`, keys sorted.
#[derive(Debug, Default, Clone, Copy)]
pub struct JsonCodec;

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct JsonDocument<V> {
    version: u32,
    values: BTreeMap<String, V>,
}

impl Codec for JsonCodec {
    fn decode(&self, bytes: &[u8]) -> Result<BTreeMap<String, Field>, String> {
        let doc: JsonDocument<serde_json::Value> =
            serde_json::from_slice(bytes).map_err(|_| "not a valid settings document")?;
        if doc.version != 1 {
            return Err("unsupported settings version".into());
        }
        Ok(doc
            .values
            .into_iter()
            .map(|(k, v)| {
                let field = match v {
                    serde_json::Value::Number(n) => n.as_i64().map_or(Field::Other, Field::Int),
                    serde_json::Value::Bool(b) => Field::Bool(b),
                    serde_json::Value::String(s) => Field::Text(s),
                    _ => Field::Other,
                };
                (k, field)
            })
            .collect())
    }

    fn encode(&self, values: &[(&Spec, i32)]) -> Result<Vec<u8>, String> {
        let values = values.iter().map(|(s, v)| (s.key.to_owned(), *v)).collect();
        serde_json::to_vec(&JsonDocument { version: 1, values })
            .map_err(|_| "cannot encode settings".into())
    }
}

/// Reolink's format: one `key = value` per setting, a switch as a boolean, a select as its option
/// text, keys sorted, after an optional comment header.
#[cfg(feature = "toml")]
#[derive(Debug, Default, Clone, Copy)]
pub struct TomlCodec {
    /// Written first, e.g. `"# Written by cam; edits while it runs are overwritten.\n"`.
    pub header: &'static str,
}

#[cfg(feature = "toml")]
impl Codec for TomlCodec {
    fn decode(&self, bytes: &[u8]) -> Result<BTreeMap<String, Field>, String> {
        let text = std::str::from_utf8(bytes).map_err(|_| "not UTF-8")?;
        // TOML errors may quote input lines; keep only the fact.
        let table: toml::Table = toml::from_str(text).map_err(|_| "not valid TOML")?;
        Ok(table
            .into_iter()
            .map(|(k, v)| {
                let field = match v {
                    toml::Value::Integer(n) => Field::Int(n),
                    toml::Value::Boolean(b) => Field::Bool(b),
                    toml::Value::String(s) => Field::Text(s),
                    _ => Field::Other,
                };
                (k, field)
            })
            .collect())
    }

    fn encode(&self, values: &[(&Spec, i32)]) -> Result<Vec<u8>, String> {
        let mut table = toml::Table::new();
        for (s, v) in values {
            let value = match s.kind {
                Kind::Number { .. } => toml::Value::Integer((*v).into()),
                Kind::Switch => toml::Value::Boolean(*v == 1),
                Kind::Select(_) => toml::Value::String(s.render(*v)),
            };
            table.insert(s.key.into(), value);
        }
        let body = toml::to_string(&table).map_err(|_| "cannot encode settings")?;
        Ok(format!("{}{body}", self.header).into_bytes())
    }
}

/// Where the settings document lives. `write` replaces it atomically or leaves it unchanged.
pub trait Disk: Send {
    /// `None` when there is no document yet.
    fn read(&mut self) -> io::Result<Option<Vec<u8>>>;
    fn write(&mut self, bytes: &[u8]) -> io::Result<()>;
}

impl Disk for Box<dyn Disk> {
    fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        (**self).read()
    }
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        (**self).write(bytes)
    }
}

/// The settings file on Linux. Reads only a regular file of at most [`MAX_FILE`] bytes, never
/// through a symlink or blocking on a FIFO; by default also only an owned, mode-0600, single-link
/// one (see [`FileDisk::any_mode`]). A file refused for its owner, mode or link count is renamed
/// aside to `<name>.rejected` (replacing an earlier one), so the next save cannot overwrite what an
/// operator may still need; if it cannot be moved, nothing is saved over it. Writes a fresh 0600
/// temp file beside it, fsyncs it, renames it over the file and fsyncs the directory.
#[derive(Debug, Clone)]
pub struct FileDisk {
    path: PathBuf,
    uid: u32,
    private_dir: bool,
    private_file: bool,
    /// A refused file could not be moved aside: writing would destroy it.
    blocked: bool,
}

impl FileDisk {
    /// Owned by the effective user.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            // SAFETY: geteuid takes no arguments and has no memory effects.
            uid: unsafe { libc::geteuid() },
            private_dir: false,
            private_file: true,
            blocked: false,
        }
    }

    /// Accept any regular file when reading (Reolink: its file may have been copied or edited by
    /// hand); writes are still 0600.
    pub fn any_mode(mut self) -> Self {
        self.private_file = false;
        self
    }

    /// Also require the directory to be a real directory owned by this user with mode 0700 (Mi 360).
    pub fn private_dir(mut self) -> Self {
        self.private_dir = true;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn parent(&self) -> io::Result<&Path> {
        let parent = match self.path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        if self.private_dir {
            let m = std::fs::symlink_metadata(parent)?;
            if !m.is_dir() || m.uid() != self.uid || m.mode() & 0o7777 != 0o700 {
                return Err(io::Error::other(
                    "settings directory must be private and owned by the runtime",
                ));
            }
        }
        Ok(parent)
    }

    fn name(&self) -> io::Result<&str> {
        self.path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| io::Error::other("invalid settings path"))
    }

    fn temp(&self, parent: &Path) -> io::Result<PathBuf> {
        Ok(parent.join(format!(".{}.tmp", self.name()?)))
    }

    /// Moves a refused file to `<name>.rejected`; the error says where it went.
    fn reject(&mut self, parent: &Path) -> io::Error {
        let moved = self.name().and_then(|name| {
            let aside = parent.join(format!("{name}.rejected"));
            std::fs::rename(&self.path, &aside)?;
            let _ = std::fs::File::open(parent).and_then(|d| d.sync_all());
            Ok(aside)
        });
        match moved {
            Ok(aside) => io::Error::other(format!(
                "settings file is not private (owner, mode 0600, one link); moved to {}",
                aside.display()
            )),
            Err(e) => {
                self.blocked = true;
                io::Error::other(format!(
                    "settings file is not private and cannot be moved aside ({e}); not saving over it"
                ))
            }
        }
    }
}

impl Disk for FileDisk {
    fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        let parent = self.parent()?.to_path_buf();
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&self.path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let m = file.metadata()?;
        let private = m.uid() == self.uid && m.mode() & 0o7777 == 0o600 && m.nlink() == 1;
        if m.is_file() && self.private_file && !private {
            drop(file);
            return Err(self.reject(&parent));
        }
        if !m.is_file() || m.len() > MAX_FILE as u64 {
            return Err(io::Error::other(
                "settings file must be private, regular and bounded",
            ));
        }
        let mut bytes = Vec::new();
        file.take(MAX_FILE as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_FILE {
            return Err(io::Error::other("settings file exceeds the limit"));
        }
        Ok(Some(bytes))
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() > MAX_FILE {
            return Err(io::Error::other("settings document exceeds the limit"));
        }
        if self.blocked {
            return Err(io::Error::other(
                "refused settings file is still in place; not saving over it",
            ));
        }
        let parent = self.parent()?;
        let temp = self.temp(parent)?;
        // A temp left by a crash is ours (one writer per file); unlinking never follows a link.
        match std::fs::remove_file(&temp) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        let mut created = false;
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&temp)?;
            created = true;
            file.write_all(bytes)?;
            file.sync_all()?;
            std::fs::rename(&temp, &self.path)?;
            std::fs::File::open(parent)?.sync_all()
        })();
        if result.is_err() && created {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
}

/// What a subscriber receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A setting's value in force changed.
    Changed { slot: u16, value: i32 },
    /// The camera is about to tear down part of the video pipeline (a VPSS rebuild); streams stop.
    PipelineRebuilding,
    /// The pipeline is back; re-read the stream parameters.
    PipelineRebuilt,
}

#[derive(Debug)]
struct FeedState {
    values: Vec<i32>,
    subscribers: Vec<Sender<Event>>,
}

#[derive(Debug)]
struct Feed {
    state: Mutex<FeedState>,
}

impl Feed {
    fn lock(&self) -> MutexGuard<'_, FeedState> {
        // A panicking subscriber cannot corrupt a Vec of values; keep serving.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn emit(state: &mut FeedState, event: Event) {
        state.subscribers.retain(|s| s.send(event).is_ok());
    }
}

/// Read access and change subscription for another owner (ONVIF). Cheap to clone, any thread.
#[derive(Debug, Clone)]
pub struct Reader<'c> {
    specs: &'c [Spec],
    feed: Arc<Feed>,
}

impl<'c> Reader<'c> {
    pub fn get(&self, slot: u16) -> Option<i32> {
        let i = self.specs.iter().position(|s| s.slot == slot)?;
        self.feed.lock().values.get(i).copied()
    }

    pub fn value(&self, key: &str) -> Option<i32> {
        let i = self.specs.iter().position(|s| s.key == key)?;
        self.feed.lock().values.get(i).copied()
    }

    /// The state text, e.g. a select's option.
    pub fn text(&self, key: &str) -> Option<String> {
        let spec = self.specs.iter().find(|s| s.key == key)?;
        self.value(key).map(|v| spec.render(v))
    }

    /// Every later [`Event`], in order; dropping the receiver unsubscribes. The queue is unbounded
    /// (settings change rarely), so a live subscriber must keep draining it.
    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = channel();
        self.feed.lock().subscribers.push(tx);
        rx
    }

    /// The current parameters of one stream, from the camera's keys for it.
    pub fn stream(&self, keys: &StreamKeys) -> StreamParams {
        // One snapshot, so a change of several keys is never seen half-applied.
        let values = self.feed.lock().values.clone();
        let at = |k: &str| {
            let i = self.specs.iter().position(|s| s.key == k)?;
            Some((&self.specs[i], values[i]))
        };
        let number = |k: Option<&str>| k.and_then(at).and_then(|(_, v)| u32::try_from(v).ok());
        let text = |k: Option<&str>| k.and_then(at).map(|(s, v)| s.render(v));
        let size = text(keys.resolution).and_then(|t| {
            let (w, h) = t.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        });
        StreamParams {
            width: size.map(|(w, _)| w),
            height: size.map(|(_, h)| h),
            fps: number(keys.fps),
            kbps: number(keys.kbps),
            gop: number(keys.gop),
            profile: text(keys.profile),
        }
    }
}

/// The setting keys that make up one stream on a camera; `None` where the camera has no such setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamKeys {
    /// A select whose options read `<width>x<height>`.
    pub resolution: Option<&'static str>,
    pub fps: Option<&'static str>,
    pub kbps: Option<&'static str>,
    pub gop: Option<&'static str>,
    /// A select; its option text is reported.
    pub profile: Option<&'static str>,
}

/// One stream's current parameters; `None` where the camera has no setting or it does not parse.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamParams {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<u32>,
    pub kbps: Option<u32>,
    pub gop: Option<u32>,
    pub profile: Option<String>,
}

/// Lets the media owner announce pipeline rebuilds on the settings feed. Cheap to clone, any thread.
#[derive(Debug, Clone)]
pub struct Announcer {
    feed: Arc<Feed>,
}

impl Announcer {
    /// Before tearing down; subscribers stop using the streams.
    pub fn rebuilding(&self) {
        Feed::emit(&mut self.feed.lock(), Event::PipelineRebuilding);
    }

    /// After the pipeline runs again.
    pub fn rebuilt(&self) {
        Feed::emit(&mut self.feed.lock(), Event::PipelineRebuilt);
    }
}

/// The values in force for one spec list, always valid, and their persistence. One owner holds it;
/// others use [`Values::reader`] and [`Values::announcer`].
pub struct Values<'c, D: Disk, C: Codec> {
    specs: &'c [Spec],
    current: Vec<i32>,
    /// Read from the document, observed or put; a default otherwise.
    set: Vec<bool>,
    /// Write only set slots (see [`Values::sparse`]).
    sparse: bool,
    disk: D,
    codec: C,
    problems: Vec<Problem>,
    first_run: bool,
    /// When the oldest unsaved change was made.
    dirty_since: Option<Duration>,
    /// When a failed write is retried.
    retry_at: Option<Duration>,
    feed: Arc<Feed>,
}

impl<'c, D: Disk, C: Codec> Values<'c, D, C> {
    /// Reads the document; never fails. Absent: defaults. Invalid document or field: that default
    /// plus a [`Problem`]. Unknown keys are ignored so a downgrade keeps working.
    pub fn load(specs: &'c [Spec], mut disk: D, codec: C) -> (Self, Vec<Problem>) {
        let mut current: Vec<i32> = specs.iter().map(|s| s.default).collect();
        let mut set = vec![false; specs.len()];
        let mut problems = Vec::new();
        let whole = |reason: String| Problem { key: None, reason };
        let bytes = disk.read().unwrap_or_else(|e| {
            problems.push(whole(format!("unreadable: {e}")));
            None
        });
        let first_run = bytes.is_none() && problems.is_empty();
        match bytes.map(|b| {
            if b.len() > MAX_FILE {
                Err("exceeds size limit".to_string())
            } else {
                codec.decode(&b)
            }
        }) {
            None => {}
            Some(Err(reason)) => problems.push(whole(reason)),
            Some(Ok(fields)) => {
                for (i, s) in specs.iter().enumerate() {
                    let Some(field) = fields.get(s.key) else {
                        continue;
                    };
                    match s.read(field) {
                        Some(v) => {
                            current[i] = v;
                            set[i] = true;
                        }
                        None => problems.push(Problem {
                            key: Some(s.key.into()),
                            reason: "invalid value".into(),
                        }),
                    }
                }
            }
        }
        let feed = Arc::new(Feed {
            state: Mutex::new(FeedState {
                values: current.clone(),
                subscribers: Vec::new(),
            }),
        });
        let values = Self {
            specs,
            current,
            set,
            sparse: false,
            disk,
            codec,
            problems: problems.clone(),
            first_run,
            dirty_since: None,
            retry_at: None,
            feed,
        };
        (values, problems)
    }

    /// Writes only the slots read from the document, observed or put, so a value that never came
    /// from the camera is never saved (Mi 360 saves only hardware-confirmed values).
    pub fn sparse(mut self) -> Self {
        self.sparse = true;
        self
    }

    /// The slot's value was read from the document, observed or put, rather than being its default.
    pub fn is_set(&self, slot: u16) -> bool {
        self.index(slot).is_ok_and(|i| self.set[i])
    }

    pub fn specs(&self) -> &'c [Spec] {
        self.specs
    }

    fn index(&self, slot: u16) -> Result<usize, Rejected> {
        self.specs
            .iter()
            .position(|s| s.slot == slot)
            .ok_or_else(|| Rejected(format!("unknown setting slot {slot}")))
    }

    pub fn spec(&self, slot: u16) -> Option<&'c Spec> {
        self.specs.iter().find(|s| s.slot == slot)
    }

    pub fn spec_by_key(&self, key: &str) -> Option<&'c Spec> {
        self.specs.iter().find(|s| s.key == key)
    }

    pub fn get(&self, slot: u16) -> Option<i32> {
        let i = self.specs.iter().position(|s| s.slot == slot)?;
        Some(self.current[i])
    }

    pub fn value(&self, key: &str) -> Option<i32> {
        let i = self.specs.iter().position(|s| s.key == key)?;
        Some(self.current[i])
    }

    /// No document existed at load: the camera may seed initial values (e.g. from an old config).
    pub fn first_run(&self) -> bool {
        self.first_run
    }

    /// Puts `value` in force. `Ok(true)` if it changed: subscribers are told and, if nothing was
    /// unsaved, the save window starts at `now` (any monotonic time). An equal value changes nothing.
    /// In sparse mode, putting a slot's default for the first time is a change to save, though not
    /// a value change (`Ok(false)`).
    pub fn put(&mut self, slot: u16, value: i32, now: Duration) -> Result<bool, Rejected> {
        let i = self.index(slot)?;
        self.specs[i].checked(value)?;
        let newly_set = self.sparse && !self.set[i];
        let changed = self.adopt(i, value);
        if changed || newly_set {
            self.dirty_since.get_or_insert(now);
        }
        Ok(changed)
    }

    /// Puts a value read back from the device in force without starting the save window: it is
    /// written with the next save, but alone causes none. `Ok(true)` if it changed.
    pub fn observe(&mut self, slot: u16, value: i32) -> Result<bool, Rejected> {
        let i = self.index(slot)?;
        self.specs[i].checked(value)?;
        Ok(self.adopt(i, value))
    }

    fn adopt(&mut self, i: usize, value: i32) -> bool {
        self.set[i] = true;
        if self.current[i] == value {
            return false;
        }
        self.current[i] = value;
        let mut feed = self.feed.lock();
        feed.values[i] = value;
        Feed::emit(
            &mut feed,
            Event::Changed {
                slot: self.specs[i].slot,
                value,
            },
        );
        true
    }

    /// Leaves the slot out of later writes in sparse mode (a capability absent on this boot) until
    /// it is observed or put again. Starts no save; the value in force is kept.
    pub fn forget(&mut self, slot: u16) {
        if let Ok(i) = self.index(slot) {
            self.set[i] = false;
        }
    }

    /// [`Values::tick`] at `now` would write.
    pub fn due(&self, now: Duration) -> bool {
        self.dirty_since
            .is_some_and(|since| now >= self.retry_at.unwrap_or(since + SAVE_WITHIN))
    }

    pub fn dirty(&self) -> bool {
        self.dirty_since.is_some()
    }

    /// Writes the values once the window of the oldest unsaved change has run out; a failure is
    /// retried one window later and reported as a problem until a write succeeds.
    pub fn tick(&mut self, now: Duration) -> io::Result<()> {
        if !self.due(now) {
            return Ok(());
        }
        let result = self.write();
        if result.is_err() {
            self.save_failed(Some(now + SAVE_WITHIN));
        }
        result
    }

    /// Writes now if anything is unsaved (orderly shutdown).
    pub fn flush(&mut self) -> io::Result<()> {
        if self.dirty_since.is_none() {
            return Ok(());
        }
        let result = self.write();
        if result.is_err() {
            self.save_failed(None);
        }
        result
    }

    fn save_failed(&mut self, retry_at: Option<Duration>) {
        if retry_at.is_some() {
            self.retry_at = retry_at;
        }
        if !self.problems.iter().any(|p| p.reason == SAVE_FAILED) {
            self.problems.push(Problem {
                key: None,
                reason: SAVE_FAILED.into(),
            });
        }
    }

    fn write(&mut self) -> io::Result<()> {
        let entries: Vec<(&Spec, i32)> = self
            .specs
            .iter()
            .zip(self.current.iter().copied())
            .zip(&self.set)
            .filter(|(_, set)| !self.sparse || **set)
            .map(|(entry, _)| entry)
            .collect();
        let bytes = self.codec.encode(&entries).map_err(io::Error::other)?;
        if bytes.len() > MAX_FILE {
            return Err(io::Error::other("settings document exceeds the limit"));
        }
        self.disk.write(&bytes)?;
        self.dirty_since = None;
        self.retry_at = None;
        self.problems.retain(|p| p.reason != SAVE_FAILED);
        Ok(())
    }

    /// Every setting with its MQTT state text, in spec order.
    pub fn states(&self) -> impl Iterator<Item = (&'c str, String)> + '_ {
        self.specs
            .iter()
            .zip(&self.current)
            .map(|(s, v)| (s.key, s.render(*v)))
    }

    /// Problems from loading, plus a failed save until the next write succeeds.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
    }

    pub fn reader(&self) -> Reader<'c> {
        Reader {
            specs: self.specs,
            feed: Arc::clone(&self.feed),
        }
    }

    pub fn announcer(&self) -> Announcer {
        Announcer {
            feed: Arc::clone(&self.feed),
        }
    }

    pub fn disk(&self) -> &D {
        &self.disk
    }

    /// For a camera's own disk controls (and tests that fail it on purpose).
    pub fn disk_mut(&mut self) -> &mut D {
        &mut self.disk
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn entity_is_a_config_entity_routed_to_its_slot() {
        use crate::mqtt::{Category, Control, Domain};
        let spec =
            Spec::number(7, "talk_db", "Talk volume", (0, 18), Some("dB"), 9).icon("mdi:phone");
        let e = spec.entity();
        assert_eq!(e.id, "talk_db");
        assert_eq!(e.category, Some(Category::Config));
        assert_eq!(e.control, Control::Slot(7));
        assert_eq!(e.unit, Some("dB"));
        assert_eq!(e.icon, Some("mdi:phone"));
        assert!(matches!(
            e.domain,
            Domain::Number {
                min: 0,
                max: 18,
                ..
            }
        ));
        let sel = Spec::select(8, "rc", "Rate control", &["CBR", "VBR"], 1).entity();
        assert_eq!(sel.domain, Domain::select(["CBR", "VBR"]));
    }
    use super::*;

    const PROFILES: &[&str] = &["baseline", "main", "high"];
    const SIZES: &[&str] = &["1920x1080", "1280x720"];
    const SPECS: &[Spec] = &[
        Spec::number(0, "mic_volume", "Mic volume", (-30, 120), None, 0),
        Spec::switch(1, "mirror", "Mirror", false),
        Spec::select(2, "main_resolution", "Main resolution", SIZES, 0).section("main_stream"),
        Spec::number(3, "main_fps", "Main FPS", (1, 30), Some("fps"), 25),
        Spec::number(
            4,
            "main_bitrate",
            "Main bitrate",
            (512, 4096),
            Some("kbit/s"),
            2048,
        ),
        Spec::select(5, "main_profile", "Main profile", PROFILES, 2).icon("mdi:video"),
    ];

    #[derive(Default)]
    struct Fake {
        file: Option<Vec<u8>>,
        fail_read: bool,
        fail_write: bool,
        writes: usize,
    }
    impl Disk for Fake {
        fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
            if self.fail_read {
                return Err(io::Error::other("fake read failure"));
            }
            Ok(self.file.clone())
        }
        fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            if self.fail_write {
                return Err(io::Error::other("fake write failure"));
            }
            self.writes += 1;
            self.file = Some(bytes.to_vec());
            Ok(())
        }
    }

    fn with_file(text: &str) -> Fake {
        Fake {
            file: Some(text.as_bytes().to_vec()),
            ..Fake::default()
        }
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn the_test_catalog_is_valid_and_bad_catalogs_are_named() {
        check_catalog(SPECS).unwrap();
        let dup = [SPECS[0], Spec::switch(0, "other", "Other", true)];
        assert!(check_catalog(&dup).unwrap_err().contains("mic_volume"));
        let bad = [Spec::number(9, "x", "X", (1, 5), None, 0)];
        assert_eq!(check_catalog(&bad).unwrap_err(), "x: invalid default");
        assert!(check_catalog(&[Spec::select(9, "y", "Y", &[], 0)]).is_err());
    }

    #[test]
    fn payloads_parse_and_render_per_kind() {
        let [mic, mirror, size, ..] = SPECS else {
            unreachable!()
        };
        assert_eq!(mic.parse("12"), Ok(12));
        assert_eq!(mic.parse(" 12.0 "), Ok(12), "HA sends floats");
        assert!(mic.parse("12.5").is_err());
        assert!(mic.parse("121").is_err());
        assert!(mic.parse("NaN").is_err());
        assert!(mic.parse("1e12").is_err());
        assert_eq!(mirror.parse("ON"), Ok(1));
        assert_eq!(mirror.parse("OFF"), Ok(0));
        assert!(mirror.parse("on").is_err());
        assert_eq!(size.parse("1280x720"), Ok(1));
        assert!(size.parse("640x360").is_err());
        assert_eq!(mic.render(-5), "-5");
        assert_eq!(mirror.render(1), "ON");
        assert_eq!(size.render(0), "1920x1080");
        assert_eq!(size.render(7), "", "invalid renders empty");
        assert!(!mirror.valid(2));
    }

    #[test]
    fn absent_file_is_a_first_run_with_defaults_and_no_problems() {
        let (v, problems) = Values::load(SPECS, Fake::default(), JsonCodec);
        assert!(problems.is_empty());
        assert!(v.first_run());
        assert_eq!(v.value("main_fps"), Some(25));
        let states: Vec<_> = v.states().collect();
        assert_eq!(states[1], ("mirror", "OFF".into()));
        assert_eq!(states[5], ("main_profile", "high".into()));
    }

    #[test]
    fn bad_fields_fall_back_alone_and_unknown_keys_are_ignored() {
        let file =
            r#"{"version":1,"values":{"mic_volume":200,"main_fps":12,"mirror":true,"gone":3}}"#;
        let (v, problems) = Values::load(SPECS, with_file(file), JsonCodec);
        assert!(!v.first_run());
        assert_eq!(v.value("mic_volume"), Some(0));
        assert_eq!(v.value("main_fps"), Some(12));
        assert_eq!(v.value("mirror"), Some(1));
        assert_eq!(
            problems,
            vec![Problem {
                key: Some("mic_volume".into()),
                reason: "invalid value".into()
            }]
        );
        assert_eq!(v.problems(), problems);
    }

    #[test]
    fn a_broken_unreadable_or_oversized_file_is_one_problem_and_defaults() {
        for disk in [
            with_file("{not json"),
            with_file(r#"{"version":2,"values":{}}"#),
            with_file(r#"{"version":1,"values":{},"extra":0}"#),
            with_file(&" ".repeat(MAX_FILE + 1)),
            Fake {
                fail_read: true,
                ..Fake::default()
            },
        ] {
            let (v, problems) = Values::load(SPECS, disk, JsonCodec);
            assert_eq!(problems.len(), 1, "{problems:?}");
            assert_eq!(problems[0].key, None);
            assert!(!v.first_run(), "a bad file is not a first run");
            assert_eq!(v.value("main_bitrate"), Some(2048));
        }
    }

    #[test]
    fn put_validates_and_an_equal_value_changes_nothing() {
        let (mut v, _) = Values::load(SPECS, Fake::default(), JsonCodec);
        assert!(v.put(99, 1, secs(0)).is_err());
        assert!(v.put(3, 31, secs(0)).is_err());
        assert_eq!(v.put(3, 25, secs(0)), Ok(false));
        assert!(!v.dirty());
        assert_eq!(v.put(3, 15, secs(0)), Ok(true));
        assert!(v.dirty());
        assert_eq!(v.get(3), Some(15));
    }

    #[test]
    fn the_save_window_starts_at_the_first_change_and_does_not_slide() {
        let (mut v, _) = Values::load(SPECS, Fake::default(), JsonCodec);
        v.put(3, 10, secs(100)).unwrap();
        v.put(3, 11, secs(108)).unwrap();
        v.tick(secs(109)).unwrap();
        assert_eq!(v.disk().writes, 0);
        v.tick(secs(110)).unwrap();
        assert_eq!(v.disk().writes, 1, "saved 10 s after the first change");
        v.tick(secs(200)).unwrap();
        assert_eq!(v.disk().writes, 1, "nothing unsaved");
        assert!(!v.dirty());
    }

    #[test]
    fn sparse_values_save_only_what_was_loaded_observed_or_put() {
        let file = with_file(r#"{"version":1,"values":{"mirror":1}}"#);
        let mut v = Values::load(SPECS, file, JsonCodec).0.sparse();
        assert!(v.is_set(1) && !v.is_set(3) && !v.is_set(0));
        assert_eq!(v.observe(3, 20), Ok(true));
        assert_eq!(v.observe(0, 120), Ok(true));
        assert!(v.observe(0, 121).is_err());
        assert!(!v.dirty(), "an observed value alone is never written");
        assert_eq!(v.put(5, 2, secs(4)), Ok(false), "the default, now set");
        assert!(v.dirty() && !v.due(secs(13)) && v.due(secs(14)));
        v.tick(secs(14)).unwrap();
        let file = String::from_utf8(v.disk().file.clone().unwrap()).unwrap();
        assert_eq!(
            file,
            r#"{"version":1,"values":{"main_fps":20,"main_profile":2,"mic_volume":120,"mirror":1}}"#
        );
        assert!(!v.is_set(2) && !v.is_set(4));
        v.forget(1);
        assert!(!v.is_set(1) && !v.dirty() && v.value("mirror") == Some(1));
        v.put(3, 21, secs(20)).unwrap();
        v.flush().unwrap();
        let file = String::from_utf8(v.disk().file.clone().unwrap()).unwrap();
        assert!(!file.contains("mirror"), "{file}");
    }

    #[test]
    fn a_failed_save_is_reported_and_retried_one_window_later() {
        let disk = Fake {
            fail_write: true,
            ..Fake::default()
        };
        let (mut v, _) = Values::load(SPECS, disk, JsonCodec);
        v.put(1, 1, secs(0)).unwrap();
        assert!(v.tick(secs(10)).is_err());
        assert_eq!(v.problems()[0].reason, "save failed");
        assert!(v.tick(secs(15)).is_ok(), "not retried inside the window");
        assert!(v.tick(secs(20)).is_err());
        assert_eq!(v.problems().len(), 1, "reported once");
        v.disk.fail_write = false;
        v.tick(secs(30)).unwrap();
        assert!(v.problems().is_empty());
        assert!(!v.dirty());
    }

    #[test]
    fn flush_writes_only_when_dirty_and_the_values_load_back() {
        let (mut v, _) = Values::load(SPECS, Fake::default(), JsonCodec);
        v.flush().unwrap();
        assert_eq!(v.disk().writes, 0);
        v.disk.fail_write = true;
        v.put(0, -4, secs(0)).unwrap();
        assert!(v.flush().is_err());
        assert_eq!(v.problems()[0].reason, "save failed");
        v.disk.fail_write = false;
        v.put(0, -5, secs(0)).unwrap();
        v.put(2, 1, secs(0)).unwrap();
        v.flush().unwrap();
        let file = v.disk().file.clone().unwrap();
        assert_eq!(
            String::from_utf8(file.clone()).unwrap(),
            r#"{"version":1,"values":{"main_bitrate":2048,"main_fps":25,"main_profile":2,"main_resolution":1,"mic_volume":-5,"mirror":0}}"#
        );
        let (back, problems) = Values::load(
            SPECS,
            Fake {
                file: Some(file),
                ..Fake::default()
            },
            JsonCodec,
        );
        assert!(problems.is_empty());
        assert_eq!(back.value("mic_volume"), Some(-5));
        assert_eq!(back.text_of("main_resolution"), "1280x720");
    }

    impl<D: Disk, C: Codec> Values<'_, D, C> {
        fn text_of(&self, key: &str) -> String {
            self.reader().text(key).unwrap()
        }
    }

    #[test]
    fn subscribers_see_changes_and_pipeline_events_in_order() {
        let (mut v, _) = Values::load(SPECS, Fake::default(), JsonCodec);
        let reader = v.reader();
        let events = reader.subscribe();
        let gone = reader.subscribe();
        drop(gone);
        let announcer = v.announcer();
        v.put(2, 1, secs(0)).unwrap();
        announcer.rebuilding();
        announcer.rebuilt();
        v.put(2, 1, secs(1)).unwrap();
        assert_eq!(
            events.try_iter().collect::<Vec<_>>(),
            vec![
                Event::Changed { slot: 2, value: 1 },
                Event::PipelineRebuilding,
                Event::PipelineRebuilt,
            ],
            "an unchanged put is not an event"
        );
        assert_eq!(reader.value("main_resolution"), Some(1));
        assert_eq!(reader.get(2), Some(1));
        assert_eq!(reader.value("nope"), None);
    }

    #[test]
    fn the_reader_reports_stream_params_from_another_thread() {
        let (mut v, _) = Values::load(SPECS, Fake::default(), JsonCodec);
        let reader = v.reader();
        v.put(2, 1, secs(0)).unwrap();
        v.put(4, 1024, secs(0)).unwrap();
        let keys = StreamKeys {
            resolution: Some("main_resolution"),
            fps: Some("main_fps"),
            kbps: Some("main_bitrate"),
            gop: None,
            profile: Some("main_profile"),
        };
        let params = std::thread::spawn(move || reader.stream(&keys))
            .join()
            .unwrap();
        assert_eq!(
            params,
            StreamParams {
                width: Some(1280),
                height: Some(720),
                fps: Some(25),
                kbps: Some(1024),
                gop: None,
                profile: Some("high".into()),
            }
        );
    }

    /// Mi 360's settings document as the camera writes it (`cameras/mi360/src/settings.rs`).
    #[test]
    fn mi360_documents_keep_loading() {
        const MI360: &[Spec] = &[
            Spec::switch(0, "mirror", "Mirror", false),
            Spec::number(2, "brightness", "Brightness", (0, 255), None, 128),
            Spec::select(
                6,
                "anti_flicker",
                "Anti-flicker",
                &["off", "50hz", "60hz"],
                0,
            ),
        ];
        let file = r#"{"version":1,"values":{"anti_flicker":1,"brightness":129,"mirror":1}}"#;
        let (mut v, problems) = Values::load(MI360, with_file(file), JsonCodec);
        assert!(problems.is_empty());
        assert_eq!(
            v.states().collect::<Vec<_>>(),
            vec![
                ("mirror", "ON".to_string()),
                ("brightness", "129".to_string()),
                ("anti_flicker", "50hz".to_string()),
            ]
        );
        v.put(2, 130, secs(0)).unwrap();
        v.flush().unwrap();
        assert_eq!(
            v.disk().file.as_deref().unwrap(),
            br#"{"version":1,"values":{"anti_flicker":1,"brightness":130,"mirror":1}}"#
        );
    }

    #[cfg(feature = "toml")]
    #[test]
    fn reolink_toml_documents_keep_loading_and_round_trip() {
        let file = "# Written by cam; edits while it runs are overwritten.\n\
                    main_fps = 15\nmain_profile = \"main\"\nmain_resolution = \"1280x720\"\n\
                    mic_volume = 70\nmirror = true\nunknown = 1\n";
        let codec = TomlCodec {
            header: "# Written by cam; edits while it runs are overwritten.\n",
        };
        let (mut v, problems) = Values::load(SPECS, with_file(file), codec);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(v.value("mirror"), Some(1));
        assert_eq!(v.value("main_profile"), Some(1));
        assert_eq!(v.value("main_resolution"), Some(1));
        v.put(3, 20, secs(0)).unwrap();
        v.flush().unwrap();
        let text = String::from_utf8(v.disk().file.clone().unwrap()).unwrap();
        assert_eq!(
            text,
            "# Written by cam; edits while it runs are overwritten.\n\
             main_bitrate = 2048\nmain_fps = 20\nmain_profile = \"main\"\n\
             main_resolution = \"1280x720\"\nmic_volume = 70\nmirror = true\n"
        );
        let (_, problems) =
            Values::load(SPECS, with_file("mic_volume = 1.5\nmirror = \"x\""), codec);
        assert_eq!(problems.len(), 2);
        let (_, problems) = Values::load(SPECS, with_file("= broken"), codec);
        assert_eq!(problems[0].reason, "not valid TOML");
    }

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let path = std::env::temp_dir().join(format!(
                "camera-transport-settings-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn file_disk_writes_atomically_private_and_replaces_a_stale_temp() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Dir::new("write");
        let path = dir.0.join("settings.json");
        let mut disk = FileDisk::new(&path).private_dir();
        assert_eq!(disk.read().unwrap(), None);
        std::fs::write(dir.0.join(".settings.json.tmp"), b"stale").unwrap();
        disk.write(b"one").unwrap();
        disk.write(b"two").unwrap();
        assert_eq!(disk.read().unwrap().as_deref(), Some(&b"two"[..]));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o600);
        let names: Vec<_> = std::fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec![std::ffi::OsString::from("settings.json")],
            "no temp left"
        );
        assert!(disk.write(&vec![b' '; MAX_FILE + 1]).is_err());
    }

    #[test]
    fn file_disk_refuses_links_shared_modes_and_oversized_files() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = Dir::new("refuse");
        let path = dir.0.join("settings.json");
        let mut disk = FileDisk::new(&path);
        symlink("absent", &path).unwrap();
        assert!(disk.read().is_err(), "symlink");
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            FileDisk::new(&path).any_mode().read().is_ok(),
            "Reolink accepts it"
        );
        assert!(disk.read().is_err(), "mode 0644");
        assert!(!path.exists(), "moved aside");
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(disk.read().is_ok());
        std::fs::hard_link(&path, dir.0.join("second")).unwrap();
        assert!(disk.read().is_err(), "two links");
        assert!(!path.exists(), "the link is moved aside");
        assert_eq!(std::fs::read(dir.0.join("second")).unwrap(), b"{}");
        std::fs::write(&path, vec![b' '; MAX_FILE + 1]).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(disk.read().is_err(), "oversized");
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            FileDisk::new(&path).private_dir().read().is_err(),
            "shared directory"
        );
    }

    #[test]
    fn values_persist_through_a_file_disk() {
        let dir = Dir::new("values");
        let path = dir.0.join("settings.json");
        let (mut v, _) = Values::load(SPECS, FileDisk::new(&path).private_dir(), JsonCodec);
        v.put(1, 1, secs(0)).unwrap();
        v.tick(secs(10)).unwrap();
        let (back, problems) = Values::load(SPECS, FileDisk::new(&path).private_dir(), JsonCodec);
        assert!(problems.is_empty());
        assert_eq!(back.value("mirror"), Some(1));
    }

    #[test]
    fn a_refused_file_is_kept_aside_and_reported_before_the_first_save() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Dir::new("rejected");
        let path = dir.0.join("settings.json");
        std::fs::write(&path, br#"{"mirror":1}"#).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (mut v, problems) = Values::load(SPECS, FileDisk::new(&path), JsonCodec);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].reason.contains("settings.json.rejected"),
            "{problems:?}"
        );
        assert_eq!(v.value("mirror"), Some(0), "defaults in force");
        v.put(1, 1, secs(0)).unwrap();
        v.tick(secs(10)).unwrap();
        assert_eq!(
            std::fs::read(dir.0.join("settings.json.rejected")).unwrap(),
            br#"{"mirror":1}"#,
            "the save did not overwrite it"
        );
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o600);
        let (_, problems) = Values::load(SPECS, FileDisk::new(&path), JsonCodec);
        assert!(problems.is_empty(), "{problems:?}");
    }
}
