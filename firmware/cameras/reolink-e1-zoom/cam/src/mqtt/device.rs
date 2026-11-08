//! Serialized MQTT hardware controls, shared by the Linux adapter and host peripheral fakes. The
//! hardware thread owns the settings store: it validates and persists every change, applies PTZ, IR,
//! LED, orientation and picture settings itself, and hands audio and main-stream changes to the media
//! thread.
use crate::{
    ir::{Auto, Hw, Ir, Mode},
    mqtt::{Command, Hardware, Move},
    ptz::{Axis, Bus, HoldEnd, HoldState, Position, Ptz, Toward},
    settings::{Disk, Group, Owner, Store},
    zoom::af::{Autofocus, Trigger},
    zoom::focus_map::{CalProgress, CalStep, Calibration, FocusMap, Outcome, Park, Pose, Source},
    zoom::zf_table::ZfTable,
    zoom::{Isp, Lens},
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

pub trait Platform: Send {
    fn reboot(&mut self) -> Result<(), String>;
}

pub struct Device<B: Bus, I: Isp, H: Hw, O: Platform> {
    ptz: Ptz<B>,
    lens: Lens<I>,
    ir: Ir<H>,
    os: O,
    zf: ZfTable,
    store: Store<Box<dyn Disk>>,
    media: Arc<crate::media_tuning::Handoff>,
    presets: crate::presets::Presets,
    /// The preset the camera stands at, for `preset/state`.
    preset: Option<String>,
    auto: Auto,
    /// Time of the latest tick; stamps setting changes (at most one poll old, so saves come early).
    now: Duration,
    last_ir: Option<Duration>,
    light: Option<u32>,
    /// Media refusals and failed hardware applies since start, for the `settings` sensor.
    refused: Vec<String>,
    /// Media reports already published.
    seen_reports: u64,
    /// A hold just ended: its final position goes out now, not at the next refresh.
    hold_ended: bool,
    af: Autofocus,
    /// The autofocus state last published.
    af_shown: String,
    /// Where every successful search found its focus, and the park point it predicts.
    map: FocusMap,
    last_park: Option<Park>,
    /// The driver rows derived from the map changed since they were installed.
    rows_pending: bool,
    last_rows: Option<Duration>,
    /// When the command being applied arrived (the hardware thread's clock); `now` is the last tick.
    received: Option<Duration>,
    /// The calibration walk in progress, when it started, and the last one's outcome.
    calibration: Option<Calibration>,
    calibration_started: Option<Duration>,
    last_calibration: Option<CalProgress>,
    _controls_lease: Option<std::fs::File>,
}

/// Driver rows changed by learning are reinstalled at most this often, and only while everything is idle.
const ROWS_WITHIN: Duration = Duration::from_secs(60);

impl<B: Bus, I: Isp, H: Hw, O: Platform> Device<B, I, H, O> {
    /// Homes once per daemon lifetime, before advertising online. Never beside the vendor `device`.
    /// Applies the persisted settings; a refused one is reported, never fatal.
    pub fn new(
        mut ptz: Ptz<B>,
        mut lens: Lens<I>,
        mut ir: Ir<H>,
        os: O,
        zf: ZfTable,
        owner: Owner,
    ) -> Result<Self, String> {
        let Owner {
            store,
            media,
            focus_map: map,
            presets,
        } = owner;
        let s = store.settings().clone();
        s.orientation()
            .apply(lens.isp_mut())
            .map_err(|e| e.to_string())?;
        let rows = map.rows(&zf).map_err(|e| e.to_string())?;
        lens.configure(&rows, zf.backlash)
            .map_err(|e| e.to_string())?;
        ptz.init().map_err(|e| e.to_string())?;
        ptz.set_ears(Some(Box::new(media.ears())));
        ptz.home().map_err(|e| e.to_string())?;
        lens.init_with(Duration::from_secs(30), &mut || {
            ptz.checkpoint()
                .map_err(|e| crate::zoom::Error::Ioctl(e.to_string()))
        })
        .map_err(|e| e.to_string())?;
        ptz.set_profile(s.motion().0);
        // The lens homes at focus 0; when the map knows the home view, start the first search from there.
        let home = map.park(ptz.position().map(|p| (p.pan, p.tilt)), 0, &zf);
        if matches!(home.source, Source::Learned { .. }) {
            lens.focus_to(home.focus).map_err(|e| e.to_string())?;
        }
        let mut refused = Vec::new();
        if let Err(e) = lens.set_zoom_speed(s.zoom_speed()) {
            refused.push(format!("zoom_speed: {e}"));
        }
        let (thresholds, hold, leds_at_night) = s.ir();
        if let Err(e) = ir.set_leds_at_night(leds_at_night) {
            refused.push(format!("ir_leds_at_night: {e}"));
        }
        let auto = Auto::new(thresholds, hold).map_err(|e| e.to_string())?;
        ir.status_led(s.led()).map_err(|e| e.to_string())?;
        let mut device = Self {
            ptz,
            lens,
            ir,
            os,
            zf,
            store,
            media,
            presets,
            preset: None,
            auto,
            now: Duration::ZERO,
            last_ir: None,
            light: None,
            refused,
            seen_reports: 0,
            hold_ended: false,
            af: Autofocus::new(s.autofocus()),
            af_shown: String::new(),
            map,
            last_park: Some(home),
            rows_pending: false,
            last_rows: None,
            received: None,
            calibration: None,
            calibration_started: None,
            last_calibration: None,
            _controls_lease: None,
        };
        if let Err(e) = device.apply_ir_mode() {
            device.refused.push(format!("ir: {e}"));
        }
        for item in crate::isp_iq::ITEMS {
            if let Some(v) = s.picture(item.key)
                && let Err(e) = crate::isp_iq::apply(device.lens.isp_mut(), item.key, v)
            {
                device.refused.push(format!("{}: {e}", item.key));
            }
        }
        for r in &device.refused {
            eprintln!("settings: {r}");
        }
        if let Err(e) = device.go_default() {
            eprintln!("ptz: could not go to the Default preset after homing: {e}");
        }
        device.af.request(Trigger::Startup, Duration::ZERO);
        device.af_shown = device.af.state().to_string();
        Ok(device)
    }

    /// Ends a home at the `Default` preset (pan, tilt and zoom), so the pose after a start or a Home is always
    /// the same one. Without a `Default` preset the head stays at the stop.
    fn go_default(&mut self) -> Result<(), String> {
        let Some(p) = self.presets.get("Default") else {
            return Ok(());
        };
        let target = crate::ptz::Position {
            pan: p.pan,
            tilt: p.tilt,
        };
        self.ptz
            .goto_at(target, Some(self.now))
            .map_err(|e| e.to_string())?;
        self.moved(Some((p.pan, p.tilt)), self.now)?;
        self.zoom(p.zoom)?;
        self.preset = Some("Default".to_owned());
        Ok(())
    }

    /// Zooms with the park focus for `n`, then asks autofocus to refine it.
    fn zoom(&mut self, n: u32) -> Result<(), String> {
        self.park(n)?;
        self.af.request(Trigger::Zoom, self.now);
        Ok(())
    }

    /// Any motion command first abandons a running search, back at the park focus for the zoom.
    fn cancel_af(&mut self) -> Result<(), String> {
        if self.af.cancel() {
            let at = self.lens.positions().map_err(|e| e.to_string())?.zoom;
            self.park(at)?;
        }
        Ok(())
    }

    /// The focus the map predicts for `zoom` where the lens points now (the calibrated one when nothing is
    /// known there or the pan/tilt is unknown).
    fn park_point(&mut self, zoom: u32) -> Park {
        let angles = self.ptz.position().map(|p| (p.pan, p.tilt));
        let park = self.map.park(angles, zoom, &self.zf);
        self.last_park = Some(park);
        park
    }

    /// A pan/tilt ended at `angles` (the commanded target, or the confirmed position after a hold): when the map
    /// knows a sharper focus for this view than the lens holds, go there first (the search then climbs from near
    /// the peak); otherwise the move check decides.
    fn moved(&mut self, angles: Option<(u32, u32)>, now: Duration) -> Result<(), String> {
        let zoom = self.lens.positions().map_err(|e| e.to_string())?;
        let park = self.map.park(angles, zoom.zoom, &self.zf);
        self.last_park = Some(park);
        if matches!(park.source, Source::Learned { .. }) && park.focus.abs_diff(zoom.focus) > 6 {
            self.lens.focus_to(park.focus).map_err(|e| e.to_string())?;
        }
        self.af.request(Trigger::Move, now);
        Ok(())
    }

    /// Hands a finished `Focused` search to the map at the confirmed pan/tilt; rows widen when it changed them.
    fn learn(&mut self, now: Duration) {
        let Some((zoom, focus, va)) = self.af.take_learned() else {
            return;
        };
        let Some(p) = self.ptz.position() else {
            return;
        };
        let before = self.map.rows(&self.zf).ok();
        self.map.learn(
            Pose {
                pan: p.pan,
                tilt: p.tilt,
                zoom,
            },
            focus,
            va,
            now,
        );
        if self.map.rows(&self.zf).ok() != before {
            self.rows_pending = true;
        }
    }

    /// The sensor state: the walk's progress and the minutes it still needs while it runs, else the autofocus
    /// state.
    fn af_state(&self) -> String {
        match self.calibration.as_ref().map(|c| c.progress()) {
            Some(p) if p.state == crate::zoom::focus_map::CalState::Running => {
                let left = self.calibration_remaining().as_secs().div_ceil(60);
                format!("calibrating {}/{}, {left} min left", p.done, p.total)
            }
            _ => self.af.state().to_string(),
        }
    }

    /// Time the running walk still needs: the nominal cost per point for the first points (they lie near the
    /// start and go quickly), then the walk's own pace.
    fn calibration_remaining(&self) -> Duration {
        use crate::zoom::focus_map::{PACE_AFTER, SECONDS_PER_POINT};
        let Some(p) = self.calibration.as_ref().map(|c| c.progress()) else {
            return Duration::ZERO;
        };
        let left = (p.total - p.done) as u64;
        let elapsed = self
            .calibration_started
            .map_or(Duration::ZERO, |t| self.now.saturating_sub(t));
        if p.done < PACE_AFTER {
            Duration::from_secs(left * SECONDS_PER_POINT)
        } else {
            elapsed / p.done as u32 * left as u32
        }
    }

    /// Any command but a setting, a reboot or another press ends a running walk where it is.
    fn cancel_calibration(&mut self) {
        if let Some(c) = self.calibration.as_mut()
            && !c.finished()
        {
            c.cancel();
            self.last_calibration = Some(c.progress());
            self.calibration = None;
        }
    }

    /// One step of the calibration walk per tick, only while the lens, the pan/tilt and autofocus are idle: a
    /// goto plus a park (the cost of one preset), or a search request, or the return to the start.
    fn calibrate(&mut self, now: Duration) -> Result<(), String> {
        let Some(cal) = self.calibration.as_mut() else {
            return Ok(());
        };
        if cal.finished() {
            self.last_calibration = Some(cal.progress());
            self.calibration = None;
            return Ok(());
        }
        if self.ptz.holding() || self.lens.holding() || self.af.searching() {
            return Ok(());
        }
        let last = if cal.searching() {
            Some(match self.af.state() {
                crate::zoom::af::AfState::Focused(_) => Outcome::Focused,
                _ => Outcome::Skipped,
            })
        } else {
            None
        };
        let step = cal.next(last);
        self.last_calibration = Some(cal.progress());
        match step {
            CalStep::Goto(p) => {
                let target = Position {
                    pan: p.pan,
                    tilt: p.tilt,
                };
                if let Err(e) = self.ptz.goto(target) {
                    self.cancel_calibration();
                    return Err(e.to_string());
                }
                self.last_park = None;
                self.park(p.zoom)
            }
            CalStep::Search => {
                self.af.request(Trigger::Calibration, now);
                Ok(())
            }
            CalStep::Return(p) => {
                let target = Position {
                    pan: p.pan,
                    tilt: p.tilt,
                };
                if let Err(e) = self.ptz.goto(target) {
                    self.cancel_calibration();
                    return Err(e.to_string());
                }
                self.zoom(p.zoom)
            }
            CalStep::Done => Ok(()),
        }
    }

    /// Reinstalls the derived rows once learning changed them, only while nothing moves or searches.
    fn reinstall_rows(&mut self, now: Duration) -> Result<(), String> {
        if !self.rows_pending
            || self.af.searching()
            || self.ptz.holding()
            || self.lens.holding()
            || self.ptz.position().is_none()
            || self
                .last_rows
                .is_some_and(|t| now.saturating_sub(t) < ROWS_WITHIN)
        {
            return Ok(());
        }
        if self.lens.busy().map_err(|e| e.to_string())? {
            return Ok(());
        }
        self.last_rows = Some(now);
        let rows = self.map.rows(&self.zf).map_err(|e| e.to_string())?;
        self.lens
            .configure(&rows, self.zf.backlash)
            .map_err(|e| e.to_string())?;
        self.rows_pending = false;
        Ok(())
    }

    fn park(&mut self, n: u32) -> Result<(), String> {
        let focus = self.park_point(n).focus;
        let now = self.now;
        self.lens
            .zoom_focus_at(
                n,
                focus,
                Duration::from_secs(30),
                &mut || {
                    self.ptz
                        .checkpoint()
                        .map_err(|e| crate::zoom::Error::Ioctl(e.to_string()))
                },
                Some(now),
            )
            .map_err(|e| e.to_string())
    }

    /// Waits out a zoom hold's running step (any motion command ends a hold first).
    fn end_zoom_hold(&mut self) -> Result<(), String> {
        if !self.lens.holding() {
            return Ok(());
        }
        self.lens.release_hold();
        for _ in 0..3000 {
            match self.lens.poll_hold(self.now).map_err(|e| e.to_string())? {
                HoldState::Moving(d) => std::thread::sleep(d),
                _ => break,
            }
        }
        let at = self.lens.positions().map_err(|e| e.to_string())?.zoom;
        self.zoom(at)
    }

    fn apply_ir_mode(&mut self) -> Result<(), String> {
        let s = self.store.settings();
        let (thresholds, hold, _) = s.ir();
        let mode = match s.ir_mode() {
            "day" => Mode::Day,
            "night" => Mode::Night,
            _ => {
                self.auto = Auto::new(thresholds, hold).map_err(|e| e.to_string())?;
                self.last_ir = None;
                return Ok(());
            }
        };
        self.ir.set(mode).map_err(|e| e.to_string())?;
        self.day_night();
        Ok(())
    }

    /// The IR-cut filter and ISP mode switched: refocus once exposure caught up.
    fn day_night(&mut self) {
        self.af.request(
            Trigger::DayNight,
            self.now + crate::zoom::af::DAY_NIGHT_SETTLE,
        );
    }

    /// Puts a validated change into force; on a hardware refusal the old value comes back.
    fn setting(&mut self, key: &str, text: &str) -> Result<(), String> {
        let spec = crate::settings::spec(key).ok_or("unknown setting")?;
        let previous = self.store.settings().get(key);
        let Some(change) = self
            .store
            .set(key, text, self.now)
            .map_err(|e| e.to_string())?
        else {
            return Ok(());
        };
        let s = self.store.settings().clone();
        let applied = match change.spec.group {
            Group::Audio => {
                self.media.request_audio(s.audio());
                Ok(())
            }
            Group::MainStream => {
                self.media.request_main(s.main_stream());
                Ok(())
            }
            Group::Orientation => s
                .orientation()
                .apply(self.lens.isp_mut())
                .map_err(|e| e.to_string()),
            Group::Picture => match s.picture(key) {
                Some(v) => crate::isp_iq::apply(self.lens.isp_mut(), key, v)
                    .map(|_| ())
                    .map_err(|e| e.to_string()),
                None => Err("not a picture control".into()),
            },
            Group::Ir => {
                let (thresholds, hold, leds) = s.ir();
                self.ir
                    .set_leds_at_night(leds)
                    .map_err(|e| e.to_string())
                    .and_then(|()| {
                        if key == "ir" {
                            self.apply_ir_mode()
                        } else {
                            // New thresholds or hold time take effect at the next auto tick.
                            self.auto
                                .retune(thresholds, hold)
                                .map_err(|e| e.to_string())
                        }
                    })
            }
            Group::Led => self.ir.status_led(s.led()).map_err(|e| e.to_string()),
            Group::Motion => {
                self.ptz.set_profile(s.motion().0);
                Ok(())
            }
            Group::Lens => self
                .lens
                .set_zoom_speed(s.zoom_speed())
                .map_err(|e| e.to_string()),
            Group::Autofocus => {
                self.af.retune(s.autofocus());
                Ok(())
            }
        };
        if let Err(e) = &applied
            && let Some(old) = previous
        {
            let _ = self.store.put(spec, old, self.now);
            self.refused.push(format!("{key}: {e}"));
        }
        applied
    }

    /// Saves where the camera stands as `name`'s position; nothing moves and a running search goes on. Refused
    /// while the pose is not settled: a hold runs, the pan/tilt is unknown, or the calibration walk runs.
    fn save_preset(&mut self, name: &str) -> Result<(), String> {
        if self.ptz.holding() || self.lens.holding() {
            return Err("preset not saved: the camera is moving".into());
        }
        if self.calibration.as_ref().is_some_and(|c| !c.finished()) {
            return Err("preset not saved: the focus calibration is running".into());
        }
        let p = self
            .ptz
            .position()
            .ok_or("preset not saved: pan/tilt position unknown; restart to home")?;
        let zoom = self.lens.positions().map_err(|e| e.to_string())?.zoom;
        self.presets
            .save(
                name,
                crate::presets::Preset {
                    pan: p.pan,
                    tilt: p.tilt,
                    zoom,
                },
            )
            .map_err(|e| format!("preset not saved: {e}"))?;
        self.preset = Some(name.to_owned());
        Ok(())
    }

    fn start_velocity(&mut self, velocity: crate::motion_control::Velocity) -> Result<(), String> {
        crate::motion_control::Capabilities {
            zoom: true,
            coupled_pan_tilt: false,
        }
        .validate(velocity)?;
        let settings = self.store.settings();
        let (base, _) = settings.motion();
        let age = self
            .received
            .map_or(Duration::ZERO, |r| self.now.saturating_sub(r));
        let timeout = Duration::from_secs(2)
            .checked_sub(age)
            .filter(|d| !d.is_zero())
            .ok_or("stale velocity command")?;
        let orientation = settings.orientation();
        let signs = [
            if orientation.mirror() { -1 } else { 1 },
            if orientation.flip() { -1 } else { 1 },
        ];
        let targets = [velocity.pan, velocity.tilt].map(|v| {
            if v == 0 {
                None
            } else {
                let max = (base.max_pps * u32::from(v.unsigned_abs()) / 100).max(100);
                Some((
                    if v > 0 { Toward::Max } else { Toward::Zero },
                    crate::ptz::Profile::new(base.start_pps, max, base.ramp),
                ))
            }
        });
        let mut targets = targets;
        for i in 0..2 {
            if signs[i] < 0
                && let Some((toward, p)) = targets[i]
            {
                targets[i] = Some((
                    if toward == Toward::Max {
                        Toward::Zero
                    } else {
                        Toward::Max
                    },
                    p,
                ));
            }
        }
        self.ptz
            .begin_holds(targets, timeout, self.now)
            .map_err(|e| e.to_string())?;
        if velocity.zoom == 0 {
            self.lens.release_hold();
        } else {
            let requested = (settings.zoom_speed() * u32::from(velocity.zoom.unsigned_abs()) / 100)
                .clamp(20, 100);
            let rate = (requested / 20).max(1) * 20;
            self.lens
                .set_zoom_speed_if_changed(rate)
                .map_err(|e| e.to_string())?;
            self.lens
                .begin_hold(velocity.zoom > 0, timeout, self.now)
                .map_err(|e| e.to_string())?;
        }
        self.preset = None;
        Ok(())
    }
    fn start_move(&mut self, m: Move) -> Result<(), String> {
        let (_, timeout) = self.store.settings().motion();
        let profile = self.ptz.profile();
        let pan_tilt = |axis, toward| Some((axis, toward));
        let target = match m {
            // Same senses as the card's relative taps: ← pan −, → pan +, ↑ tilt +, ↓ tilt −.
            Move::Left => pan_tilt(Axis::Pan, Toward::Zero),
            Move::Right => pan_tilt(Axis::Pan, Toward::Max),
            Move::Up => pan_tilt(Axis::Tilt, Toward::Max),
            Move::Down => pan_tilt(Axis::Tilt, Toward::Zero),
            Move::ZoomIn | Move::ZoomOut => None,
            Move::Stop => {
                self.ptz.release_hold();
                self.lens.release_hold();
                return Ok(());
            }
        };
        self.preset = None;
        match target {
            Some((axis, toward)) => {
                self.end_zoom_hold()?;
                self.ptz
                    .begin_hold(axis, toward, profile, timeout, self.now)
                    .map_err(|e| e.to_string())
            }
            None => {
                self.ptz.release_hold();
                self.lens
                    .begin_hold(m == Move::ZoomIn, timeout, self.now)
                    .map_err(|e| e.to_string())
            }
        }
    }
}

impl<B: Bus + Send, I: Isp + Send, H: Hw + Send, O: Platform> Hardware for Device<B, I, H, O> {
    fn failed(&self) -> bool {
        self.ir.failed()
    }
    fn stop_endpoint(&self) -> Option<std::path::PathBuf> {
        self.ptz.stop_endpoint()
    }
    fn motion_epoch(&self) -> Option<std::sync::Arc<std::sync::atomic::AtomicU32>> {
        self.ptz.motion_epoch()
    }
    fn apply_at(&mut self, c: &Command, received: Duration) -> Result<(), String> {
        self.received = Some(received);
        let r = self.apply(c);
        self.received = None;
        r
    }
    fn apply(&mut self, c: &Command) -> Result<(), String> {
        if !matches!(
            c,
            Command::Setting(..)
                | Command::Reboot
                | Command::CalibrateFocus
                | Command::SavePreset(_)
                | Command::RemovePreset(_)
        ) {
            self.cancel_calibration();
        }
        if !matches!(
            c,
            Command::Setting(..)
                | Command::Reboot
                | Command::Refocus
                | Command::SavePreset(_)
                | Command::RemovePreset(_)
        ) {
            self.cancel_af()?;
        }
        if let Command::CalibrateFocus = c {
            if self.calibration.as_ref().is_some_and(|c| !c.finished()) {
                return Ok(());
            }
            let p = self
                .ptz
                .position()
                .ok_or("pan/tilt position unknown; restart to home")?;
            let zoom = self.lens.positions().map_err(|e| e.to_string())?.zoom;
            self.end_zoom_hold()?;
            self.preset = None;
            self.calibration = Some(Calibration::new(Pose {
                pan: p.pan,
                tilt: p.tilt,
                zoom,
            }));
            self.calibration_started = Some(self.now);
            return Ok(());
        }
        match c {
            Command::Pan(_)
            | Command::Tilt(_)
            | Command::Zoom(_)
            | Command::PanBy(_)
            | Command::TiltBy(_)
            | Command::ZoomBy(_)
            | Command::Position3d(_)
            | Command::Preset(_) => {
                self.preset = None;
                self.end_zoom_hold()?;
                let mut pos = self
                    .ptz
                    .position()
                    .ok_or("pan/tilt position unknown; restart to home")?;
                match c {
                    Command::Pan(n) => pos.pan = *n,
                    Command::Tilt(n) => pos.tilt = *n,
                    Command::Zoom(n) => return self.zoom(*n),
                    Command::PanBy(n) => {
                        pos.pan = pos.pan.saturating_add_signed(*n).min(crate::ptz::PAN_MAX)
                    }
                    Command::TiltBy(n) => {
                        pos.tilt = pos.tilt.saturating_add_signed(*n).min(crate::ptz::TILT_MAX)
                    }
                    Command::ZoomBy(n) => {
                        let current = self.lens.positions().map_err(|e| e.to_string())?.zoom;
                        return self
                            .zoom(current.saturating_add_signed(*n).min(crate::mqtt::ZOOM_MAX));
                    }
                    Command::Preset(name) => {
                        let p = self.presets.get(name).ok_or("unknown preset")?;
                        pos = Position {
                            pan: p.pan,
                            tilt: p.tilt,
                        }
                    }
                    Command::Position3d(req) => {
                        let zoom = self.lens.positions().map_err(|e| e.to_string())?.zoom;
                        // The tap was made on the picture shown `tap_delay` before it arrived: resolve it
                        // against the pose of that moment, which the recorded moves know.
                        let shown = self
                            .received
                            .map(|r| r.saturating_sub(self.store.settings().tap_delay()));
                        let (at, zoom_shown) = match shown {
                            Some(t) => (
                                self.ptz.pose_at(t).unwrap_or(pos),
                                self.lens.zoom_at(t).unwrap_or(zoom),
                            ),
                            None => (pos, zoom),
                        };
                        let target = crate::position3d::Optics::E1_ZOOM.target(
                            req,
                            at,
                            zoom_shown,
                            self.store.settings().orientation(),
                            (crate::ptz::PAN_MAX, crate::ptz::TILT_MAX),
                            crate::mqtt::ZOOM_MAX,
                        );
                        self.ptz
                            .goto_at(target.position, Some(self.now))
                            .map_err(|e| e.to_string())?;
                        self.moved(Some((target.position.pan, target.position.tilt)), self.now)?;
                        return match target.zoom {
                            Some(z) if z != zoom => self.zoom(z),
                            _ => Ok(()),
                        };
                    }
                    _ => {}
                }
                // An absolute move ends a hold first (after its running chunk).
                self.ptz
                    .goto_at(pos, Some(self.now))
                    .map_err(|e| e.to_string())?;
                self.moved(Some((pos.pan, pos.tilt)), self.now)?;
                if let Command::Preset(name) = c
                    && let Some(p) = self.presets.get(name)
                {
                    self.zoom(p.zoom)?;
                    self.preset = Some(name.clone());
                }
            }
            Command::SavePreset(name) => self.save_preset(name)?,
            Command::RemovePreset(name) => {
                self.presets
                    .remove(name)
                    .map_err(|e| format!("preset not removed: {e}"))?;
                if self.preset.as_deref() == Some(name.as_str()) {
                    self.preset = None;
                }
            }
            Command::Setting(key, text) => self.setting(key, text)?,
            Command::Move(m) => self.start_move(*m)?,
            Command::Velocity(v) => self.start_velocity(*v)?,
            Command::Home => {
                self.end_zoom_hold()?;
                self.ptz.home().map_err(|e| e.to_string())?;
                self.preset = None;
                self.go_default()?;
            }
            Command::Refocus => self.af.request(Trigger::Button, self.now),
            Command::CalibrateFocus => {}
            Command::Focus(n) => {
                self.preset = None;
                self.end_zoom_hold()?;
                self.lens.focus_to(*n).map_err(|e| e.to_string())?;
            }
            Command::Reboot => {
                let _ = self.store.flush();
                let _ = self.map.flush();
                self.os.reboot()?
            }
            Command::Stop => {
                self.preset = None;
                self.lens.release_hold();
                self.ptz.stop().map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    fn tick(&mut self, now: Duration) -> Result<Option<Duration>, String> {
        self.now = now;
        if let Err(e) = self.ptz.checkpoint() {
            self.preset = None;
            return Err(e.to_string());
        }
        let mut wait: Option<Duration> = None;
        let mut sooner = |d: Duration| wait = Some(wait.map_or(d, |w| w.min(d)));
        let mut errors = Vec::new();
        if self.ptz.holding() {
            match self.ptz.poll_hold(now) {
                Ok(HoldState::Moving(d)) => sooner(d),
                Ok(HoldState::Ended(end)) => {
                    self.hold_ended = true;
                    let angles = self.ptz.position().map(|p| (p.pan, p.tilt));
                    if let Err(e) = self.moved(angles, now) {
                        errors.push(e);
                    }
                    if end == HoldEnd::Timeout {
                        eprintln!("ptz: hold ended without a release (timeout)");
                    }
                }
                Ok(HoldState::Idle) => {}
                Err(e) => errors.push(e.to_string()),
            }
        }
        if self.lens.holding() {
            match self.lens.poll_hold(now) {
                Ok(HoldState::Moving(d)) => sooner(d),
                Ok(HoldState::Ended(_)) => {
                    self.hold_ended = true;
                    if let Err(e) = self
                        .lens
                        .set_zoom_speed_if_changed(self.store.settings().zoom_speed())
                    {
                        errors.push(e.to_string());
                    }
                    // Pan/tilt completion will settle focus at the combined pose;
                    // never block its next chunk on a zoom-end focus operation.
                    if !self.ptz.holding() {
                        match self.lens.positions() {
                            Ok(p) => {
                                if let Err(e) = self.zoom(p.zoom) {
                                    errors.push(e);
                                }
                            }
                            Err(e) => errors.push(e.to_string()),
                        }
                    }
                }
                Ok(HoldState::Idle) => {}
                Err(e) => errors.push(e.to_string()),
            }
        }
        if self
            .last_ir
            .is_none_or(|t| now.saturating_sub(t) >= crate::ir::POLL)
        {
            self.last_ir = Some(now);
            match self.ir.light() {
                Ok(reading) => {
                    self.light = Some(reading);
                    if self.store.settings().ir_mode() == "auto" {
                        // Commit the auto state only after the physical switch succeeds; a failure retries next tick.
                        let mut next = self.auto.clone();
                        match next.feed(reading, now) {
                            Some(mode) => match self.ir.set(mode) {
                                Ok(()) => {
                                    self.auto = next;
                                    self.day_night();
                                }
                                Err(e) => errors.push(e.to_string()),
                            },
                            None => self.auto = next,
                        }
                    }
                }
                Err(e) => {
                    self.light = None;
                    errors.push(e.to_string());
                }
            }
        }
        let motion_idle = !self.ptz.holding() && !self.lens.holding();
        let angles = self.ptz.position().map(|p| (p.pan, p.tilt));
        let (map, zf) = (&self.map, &self.zf);
        let park = |zoom: u32| map.park(angles, zoom, zf).focus;
        match self
            .af
            .poll(&mut self.lens, &self.zf, &park, now, motion_idle)
        {
            Ok(Some(d)) => sooner(d),
            Ok(None) => {}
            Err(e) => errors.push(format!("autofocus: {e}")),
        }
        self.learn(now);
        if let Err(e) = self.calibrate(now) {
            errors.push(format!("calibration: {e}"));
        }
        if self.calibration.is_some() {
            sooner(Duration::ZERO);
        }
        if let Err(e) = self.reinstall_rows(now) {
            errors.push(format!("focus rows: {e}"));
        }
        let refusals = self.media.take_refusals();
        if refusals.iter().any(|r| r.starts_with("main stream"))
            && let Some(main) = self.media.applied().1
        {
            self.store.adopt_main(&main, now);
        }
        for r in refusals {
            eprintln!("settings: media refused {r}");
            self.refused.push(r);
        }
        for ms in self.media.take_restarts() {
            eprintln!("settings: main stream restarted in {ms} ms");
        }
        if let Err(e) = self.store.tick(now) {
            errors.push(format!("settings save failed: {e}"));
        }
        if let Err(e) = self.map.tick(now) {
            errors.push(format!("focus map save failed: {e}"));
        }
        self.refused.truncate(16);
        if errors.is_empty() {
            Ok(wait)
        } else {
            Err(errors.join("; "))
        }
    }
    fn states(&mut self) -> Result<BTreeMap<String, String>, String> {
        let mut s = BTreeMap::new();
        if let Some(p) = self.ptz.position() {
            s.insert("pan".into(), p.pan.to_string());
            s.insert("tilt".into(), p.tilt.to_string());
        }
        s.insert(
            "zoom".into(),
            self.lens
                .positions()
                .map_err(|e| e.to_string())?
                .zoom
                .to_string(),
        );
        if let Some(p) = &self.preset {
            s.insert("preset".into(), p.clone());
        }
        // Audio and main-stream states are what the media thread reports it runs.
        let (audio, main) = self.media.applied();
        let shown = self
            .store
            .settings()
            .with_applied(audio.as_ref(), main.as_ref());
        for (k, v) in shown.states() {
            s.insert(k.into(), v);
        }
        s.insert(
            "light".into(),
            self.light.map(|l| l.to_string()).unwrap_or_default(),
        );
        let mut summary = crate::settings::summary(&self.store.problems());
        if let Some(r) = self.refused.last() {
            summary = format!("{summary}; refused {r}");
        }
        s.insert("settings".into(), summary);
        let state = self.af_state();
        s.insert(
            "autofocus_attributes".into(),
            af_attributes(
                &self.af.report(),
                self.map.points(),
                self.last_park,
                self.rows_pending,
                self.last_calibration,
                self.calibration
                    .as_ref()
                    .map(|_| self.calibration_remaining().as_secs()),
            ),
        );
        self.af_shown = state.clone();
        s.insert("autofocus_state".into(), state);
        Ok(s)
    }
    fn preset_names(&self) -> Option<Vec<String>> {
        Some(self.presets.names())
    }

    fn changed(&mut self) -> bool {
        let n = self.media.reports();
        let media = std::mem::replace(&mut self.seen_reports, n) != n;
        let af = self.af_state() != self.af_shown;
        std::mem::take(&mut self.hold_ended) || media || af
    }
    fn shutdown(&mut self) {
        if let Err(e) = self.store.flush() {
            eprintln!("settings: save at shutdown failed: {e}");
        }
        if let Err(e) = self.map.flush() {
            eprintln!("focus map: save at shutdown failed: {e}");
        }
    }
}

/// The `autofocus_state` sensor's attributes: what triggered the last search, how long it took, the watchdog's
/// reference and the current VA, the raw samples behind them, and the focus map's size and last park point.
fn af_attributes(
    r: &crate::zoom::af::AfReport,
    map_points: usize,
    park: Option<Park>,
    rows_pending: bool,
    calibration: Option<CalProgress>,
    remaining_s: Option<u64>,
) -> String {
    serde_json::json!({
        "calibration": calibration.map(|c| serde_json::json!({
            "state": c.state.as_str(),
            "done": c.done,
            "total": c.total,
            "learned": c.learned,
            "skipped": c.skipped,
            "remaining_s": remaining_s,
        })),
        "trigger": r.trigger.map(|t| t.as_str()),
        "duration_ms": r.duration.map(|d| d.as_millis() as u64),
        "reference_va": r.reference,
        "va": r.va,
        "samples": r.samples,
        "history": r.history,
        "searches": r.searches,
        "map_points": map_points,
        "park": park.map(|p| p.source.as_str()),
        "park_focus": park.map(|p| p.focus),
        "rows_pending": rows_pending,
    })
    .to_string()
}

pub struct LinuxPlatform;
impl Platform for LinuxPlatform {
    fn reboot(&mut self) -> Result<(), String> {
        let status = std::process::Command::new("/sbin/reboot")
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err("reboot failed".into())
        }
    }
}
/// [`open`] for the runtimes, which open the hardware inside their MQTT worker.
pub fn opener() -> crate::runtime::production::HardwareOpener {
    Box::new(|owner| open(owner).map(|hw| Box::new(hw) as Box<dyn Hardware>))
}

pub fn open(owner: Owner) -> Result<impl Hardware, String> {
    use crate::ownership::{self, RUNTIME_LOCK, Resource};
    let runtime = std::path::Path::new(RUNTIME_LOCK);
    let lease = ownership::exclusive(runtime, Resource::Controls).map_err(|e| e.to_string())?;
    let stop = crate::control_stop::Owner::bind(&ownership::stop_endpoint(runtime))
        .map_err(|e| e.to_string())?;
    let ptz = Ptz::new(
        crate::ptz::i2c::I2cBus::open(crate::ptz::i2c::DEVICE).map_err(|e| e.to_string())?,
    )
    .with_stop_owner(stop);
    let isp =
        crate::zoom::isp::NvtIsp::open(crate::zoom::isp::DEVICE).map_err(|e| e.to_string())?;
    let lens = Lens::new(isp);
    let ir = Ir::new(
        crate::ir::linux::LinuxHw::open(
            crate::ptz::i2c::DEVICE,
            crate::zoom::isp::DEVICE,
            crate::ir::sysfs::GPIO_ROOT,
        )
        .map_err(|e| e.to_string())?,
    );
    let sp = crate::files::read_device_or_file(
        std::path::Path::new(crate::zoom::zf_table::SP_DEVICE),
        512 * 1024,
    )
    .map_err(|e| e.to_string())?;
    let mut device = Device::new(
        ptz,
        lens,
        ir,
        LinuxPlatform,
        ZfTable::parse(&sp).map_err(|e| e.to_string())?,
        owner,
    )?;
    device._controls_lease = Some(lease);
    Ok(device)
}
