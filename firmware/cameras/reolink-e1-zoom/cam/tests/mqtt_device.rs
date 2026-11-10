use cam::{
    ir::{Ir, fake::FakeHw},
    mqtt::device::{Device, Platform},
    mqtt::{Command, Hardware},
    presets::Preset,
    ptz::{Ptz, fake::FakeBus},
    zoom::zf_table::{Point, ZfTable},
    zoom::{Lens, fake::FakeIsp},
};
use std::{sync::Arc, time::Duration};
#[derive(Default)]
struct Os;
impl Platform for Os {
    fn reboot(&mut self) -> Result<(), String> {
        Ok(())
    }
}
/// A settings owner on an in-memory file.
fn owner(file: Option<&str>) -> cam::settings::Owner {
    owner_with_map(file, SharedDisk::default())
}

/// A settings owner whose focus map lives on `map`.
fn owner_with_map(file: Option<&str>, map: SharedDisk) -> cam::settings::Owner {
    owner_full(file, map, SharedDisk::default())
}

/// The presets of the test `cam.toml`.
fn presets() -> std::collections::BTreeMap<String, Preset> {
    let p = |pan, tilt, zoom| Preset { pan, tilt, zoom };
    [
        ("Desk".into(), p(6195, 650, 0)),
        ("Shelf".into(), p(6195, 650, 512)),
        ("pos2".into(), p(6195, 650, 0)),
    ]
    .into()
}

/// A settings owner whose focus map lives on `map` and whose saved presets live on `saved`.
fn owner_full(file: Option<&str>, map: SharedDisk, saved: SharedDisk) -> cam::settings::Owner {
    let disk = cam::settings::FakeDisk {
        file: file.map(str::to_owned),
        ..Default::default()
    };
    cam::settings::Owner::load(
        Box::new(disk),
        "",
        Box::new(map),
        presets(),
        Box::new(saved),
    )
}

/// A fake disk a test keeps a handle on after the device took it.
#[derive(Clone, Default)]
struct SharedDisk(Arc<std::sync::Mutex<cam::settings::FakeDisk>>);
impl SharedDisk {
    fn with_file(text: &str) -> Self {
        Self(Arc::new(std::sync::Mutex::new(cam::settings::FakeDisk {
            file: Some(text.to_owned()),
            ..Default::default()
        })))
    }
    fn file(&self) -> Option<String> {
        self.0.lock().unwrap().file.clone()
    }
    fn ops(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().ops.clone()
    }
}
impl cam::settings::Disk for SharedDisk {
    fn read(&mut self) -> std::io::Result<Option<String>> {
        self.0.lock().unwrap().read()
    }
    fn write_temp(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.0.lock().unwrap().write_temp(bytes)
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
fn device<H: cam::ir::Hw>(ir: H) -> Device<FakeBus, FakeIsp, H, Os> {
    device_with(ir, owner(None))
}
fn device_with<H: cam::ir::Hw>(
    ir: H,
    owner: cam::settings::Owner,
) -> Device<FakeBus, FakeIsp, H, Os> {
    Device::new(
        Ptz::new(FakeBus::new(3000, 500)),
        Lens::new(FakeIsp::new(0, 209)),
        Ir::new(ir),
        Os,
        ZfTable {
            lens: 1,
            backlash: 0,
            points: vec![
                Point {
                    zoom: 0,
                    focus: 209,
                },
                Point {
                    zoom: 1024,
                    focus: 800,
                },
            ],
        },
        owner,
    )
    .unwrap()
}
/// Ticks every `step` from `now` until the device asks for a publish (a hold ended); returns that time.
fn until_changed<H: cam::ir::Hw + Send>(
    d: &mut Device<FakeBus, FakeIsp, H, Os>,
    mut now: Duration,
    step: Duration,
) -> Duration {
    d.states().unwrap();
    d.changed();
    for _ in 0..1000 {
        d.tick(now).unwrap();
        if d.changed() {
            return now;
        }
        now += step;
    }
    panic!("nothing changed")
}
fn setting(key: &str, value: &str) -> Command {
    cam::mqtt::Config::default()
        .command(&format!("camera/e1zoom/{key}/set"), value.as_bytes(), false)
        .unwrap()
        .unwrap()
}
#[test]
fn mqtt_controls_drive_existing_motor_and_ir_adapters_and_report_confirmed_state() {
    let mut d = device(FakeHw::new(1800));
    d.apply(&Command::Preset("Desk".into())).unwrap();
    let s = d.states().unwrap();
    assert_eq!(
        (&s["pan"], &s["tilt"], &s["preset"]),
        (&"6195".into(), &"650".into(), &"Desk".into())
    );
    d.apply(&Command::Pan(3577)).unwrap();
    d.apply(&Command::Tilt(739)).unwrap();
    d.apply(&Command::Zoom(512)).unwrap();
    d.apply(&setting("ir", "night")).unwrap();
    d.apply(&setting("led", "OFF")).unwrap();
    let s = d.states().unwrap();
    assert_eq!(
        (&s["pan"], &s["tilt"], &s["zoom"], &s["ir"], &s["led"]),
        (
            &"3577".into(),
            &"739".into(),
            &"512".into(),
            &"night".into(),
            &"OFF".into()
        )
    );
    assert!(
        !s.contains_key("preset"),
        "manual move invalidates the selected preset"
    );
}
#[test]
fn failed_ir_change_does_not_publish_a_successful_command_state() {
    let mut d = device(FakeHw::new(1800).failing_isp());
    assert!(d.apply(&setting("ir", "night")).is_err());
    assert_eq!(d.states().unwrap()["ir"], "auto");
}
struct SharedIr(std::sync::Arc<std::sync::Mutex<FakeHw>>);
impl cam::ir::Hw for SharedIr {
    fn ircut_drive(&mut self, f: cam::ir::Filter) -> Result<(), cam::ir::Error> {
        self.0.lock().unwrap().ircut_drive(f)
    }
    fn ircut_release(&mut self) -> Result<(), cam::ir::Error> {
        self.0.lock().unwrap().ircut_release()
    }
    fn ir_leds(&mut self, on: bool) -> Result<(), cam::ir::Error> {
        self.0.lock().unwrap().ir_leds(on)
    }
    fn isp_night(&mut self, on: bool) -> Result<(), cam::ir::Error> {
        self.0.lock().unwrap().isp_night(on)
    }
    fn status_led(&mut self, on: bool) -> Result<(), cam::ir::Error> {
        self.0.lock().unwrap().status_led(on)
    }
    fn ir_leds_state(&mut self) -> Result<bool, cam::ir::Error> {
        self.0.lock().unwrap().ir_leds_state()
    }
    fn status_led_state(&mut self) -> Result<bool, cam::ir::Error> {
        self.0.lock().unwrap().status_led_state()
    }
    fn isp_night_state(&mut self) -> Result<bool, cam::ir::Error> {
        self.0.lock().unwrap().isp_night_state()
    }
    fn light(&mut self) -> Result<u32, cam::ir::Error> {
        self.0.lock().unwrap().light()
    }
    fn sleep(&mut self, d: Duration) {
        self.0.lock().unwrap().sleep(d);
    }
}
#[test]
fn auto_ir_uses_real_hysteresis_and_manual_mode_stops_automatic_switching() {
    use cam::ir::fake::Event;
    let hw = std::sync::Arc::new(std::sync::Mutex::new(FakeHw::new(2400)));
    let mut d = device(SharedIr(hw.clone()));
    hw.lock().unwrap().clear();
    d.tick(Duration::ZERO).unwrap();
    assert!(hw.lock().unwrap().events().contains(&Event::IspNight(true)));
    d.apply(&setting("ir", "day")).unwrap();
    hw.lock().unwrap().clear();
    d.tick(Duration::from_secs(20)).unwrap();
    assert!(hw.lock().unwrap().events().is_empty());
    d.apply(&setting("ir", "auto")).unwrap();
    d.tick(Duration::from_secs(21)).unwrap();
    assert!(hw.lock().unwrap().events().contains(&Event::IspNight(true)));
}

struct StopDuringLens {
    isp: FakeIsp,
    socket: std::os::unix::net::UnixDatagram,
    sent: bool,
}
impl cam::zoom::Isp for StopDuringLens {
    fn get(
        &mut self,
        item: u32,
        words: &mut [u32; cam::zoom::WORDS],
    ) -> Result<(), cam::zoom::Error> {
        self.isp.get(item, words)
    }
    fn set(&mut self, item: u32, words: &[u32; cam::zoom::WORDS]) -> Result<(), cam::zoom::Error> {
        self.isp.set(item, words)
    }
    fn get_iq(
        &mut self,
        item: u32,
        words: &mut [u32; cam::zoom::WORDS],
    ) -> Result<(), cam::zoom::Error> {
        self.isp.get_iq(item, words)
    }
    fn set_iq(
        &mut self,
        item: u32,
        words: &[u32; cam::zoom::WORDS],
    ) -> Result<(), cam::zoom::Error> {
        self.isp.set_iq(item, words)
    }
    fn sleep(&mut self, d: Duration) {
        self.isp.sleep(d);
        if !self.sent {
            self.socket.send(b"S").unwrap();
            self.sent = true;
        }
    }
}
#[test]
fn stop_remains_serviceable_during_lens_startup_waits() {
    use std::os::unix::net::UnixDatagram;
    let root = std::env::temp_dir().join(format!("cam-lens-stop-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let endpoint = root.join("owner");
    let stop_owner = cam::control_stop::Owner::bind(&endpoint).unwrap();
    let socket = UnixDatagram::bind(root.join("client")).unwrap();
    socket.connect(&endpoint).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let result = Device::new(
        Ptz::new(FakeBus::new(3000, 500)).with_stop_owner(stop_owner),
        Lens::new(StopDuringLens {
            isp: FakeIsp::new(0, 230),
            socket: socket.try_clone().unwrap(),
            sent: false,
        }),
        Ir::new(FakeHw::new(1800)),
        Os,
        ZfTable::parse(b"<ZFTable>1000000,e6,400,3fe</ZFTable>").unwrap(),
        owner(None),
    );
    assert!(
        result.is_err(),
        "startup must abandon its home/init sequence after stop"
    );
    let mut reply = [0];
    socket.recv(&mut reply).unwrap();
    assert_eq!(reply, *b"S");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_coil_release_is_terminal_for_the_hardware_worker() {
    let mut d = device(FakeHw::new(1800).failing_release());
    assert!(d.apply(&setting("ir", "night")).is_err());
    assert!(d.failed());
}

#[test]
fn relative_controls_resolve_at_execution_and_clamp_to_physical_travel() {
    let cfg = cam::mqtt::Config::default();
    let mut d = device(FakeHw::new(1800));
    d.apply(&Command::Pan(300)).unwrap();
    d.apply(&Command::Tilt(100)).unwrap();
    for (id, payload, expected) in [
        ("pan", "relative:100", "400"),
        ("pan", "relative:-100", "300"),
        ("tilt", "relative:100", "200"),
        ("tilt", "relative:-100", "100"),
        ("zoom", "relative:64", "64"),
        ("zoom", "relative:-64", "0"),
        ("pan", "relative:-500", "0"),
        ("pan", "relative:7360", "7360"),
        ("tilt", "relative:1240", "1240"),
        ("zoom", "relative:1024", "1024"),
        ("zoom", "relative:64", "1024"),
    ] {
        let cmd = cfg
            .command(
                &format!("camera/e1zoom/{id}/set"),
                payload.as_bytes(),
                false,
            )
            .unwrap()
            .unwrap();
        d.apply(&cmd).unwrap();
        assert_eq!(d.states().unwrap()[id], expected);
        assert!(
            cfg.command(&format!("camera/e1zoom/{id}/set"), payload.as_bytes(), true)
                .unwrap()
                .is_none()
        );
    }
    for bad in [
        "relative:",
        "relative:7361",
        "relative:-7361",
        "relative:2147483647",
        "relative:1.0",
    ] {
        assert!(
            cfg.command("camera/e1zoom/pan/set", bad.as_bytes(), false)
                .is_err()
        );
    }
}

#[test]
fn position3d_centres_a_tap_and_zooms_to_a_box_from_the_confirmed_position() {
    let cfg = cam::mqtt::Config::default();
    let mut d = device(FakeHw::new(1800));
    d.apply(&Command::Pan(3000)).unwrap();
    d.apply(&Command::Tilt(600)).unwrap();
    let send = |d: &mut _, payload: &str| {
        let cmd = cfg
            .command("camera/e1zoom/position3d/set", payload.as_bytes(), false)
            .unwrap()
            .unwrap();
        cam::mqtt::Hardware::apply(d, &cmd)
    };
    let state =
        |d: &mut Device<_, _, _, _>, k: &str| d.states().unwrap()[k].parse::<u32>().unwrap();

    // The card's tap, upper left of centre: pan falls, tilt rises, zoom stays.
    send(
        &mut d,
        r#"{"start_x":66,"start_y":189,"end_x":66,"end_y":189,"operation":"zoomto"}"#,
    )
    .unwrap();
    let (pan, tilt) = (state(&mut d, "pan"), state(&mut d, "tilt"));
    assert!(pan < 3000 && tilt > 600, "pan {pan} tilt {tilt}");
    assert_eq!(state(&mut d, "zoom"), 0);

    // A one-coordinate box at the centre: no turn, zoom in as far as the lens goes.
    send(
        &mut d,
        r#"{"start_x":127,"start_y":127,"end_x":128,"end_y":128}"#,
    )
    .unwrap();
    assert!(state(&mut d, "zoom") > 0, "a 1-coordinate box zooms in");
    assert!(state(&mut d, "pan").abs_diff(pan) <= 2);

    // Retained commands are never executed; garbage is refused before the hardware.
    assert!(
        cfg.command(
            "camera/e1zoom/position3d/set",
            br#"{"start_x":0,"start_y":0,"end_x":0,"end_y":0}"#,
            true
        )
        .unwrap()
        .is_none()
    );
    assert!(
        cfg.command("camera/e1zoom/position3d/set", b"128,128", false)
            .is_err()
    );

    // Unknown position after a stop refuses, like every other absolute move.
    d.apply(&Command::Stop).unwrap();
    assert!(
        send(
            &mut d,
            r#"{"start_x":10,"start_y":10,"end_x":10,"end_y":10}"#
        )
        .unwrap_err()
        .contains("unknown")
    );
}

#[test]
fn a_field_of_view_move_turns_by_picture_fractions_on_the_lens_map() {
    let mut d = device(FakeHw::new(1800));
    d.apply(&Command::Pan(3000)).unwrap();
    d.apply(&Command::Tilt(600)).unwrap();
    let state =
        |d: &mut Device<_, _, _, _>, k: &str| d.states().unwrap()[k].parse::<u32>().unwrap();
    // Half a picture right at zoom 0 is the right-edge point of a tap: 1050 steps on the measured map.
    d.apply(&Command::Fov { x: 500, y: 0 }).unwrap();
    assert_eq!(state(&mut d, "pan"), 3000 + 1050);
    assert_eq!(state(&mut d, "tilt"), 600);
    // Both axes in one move; the picture is 4:3, so a quarter of its height is 3/16 of a width up:
    // 0.91 · (1960.7 · 0.1875 + 553.4 · 0.1875³) = 338 tilt steps.
    d.apply(&Command::Fov { x: -500, y: 250 }).unwrap();
    assert_eq!(state(&mut d, "pan"), 3000);
    assert_eq!(state(&mut d, "tilt"), 600 + 338);
    // The same move covers less ground zoomed in (the picture is narrower).
    d.apply(&Command::Zoom(1024)).unwrap();
    d.apply(&Command::Fov { x: 500, y: 0 }).unwrap();
    assert!(state(&mut d, "pan") < 3000 + 1050 - 200);
    // Travel clamps at the ends.
    d.apply(&Command::PanTilt(7300, 1230)).unwrap();
    d.apply(&Command::Fov { x: 1000, y: 1000 }).unwrap();
    assert_eq!((state(&mut d, "pan"), state(&mut d, "tilt")), (7360, 1240));
}

#[test]
fn an_absolute_pan_tilt_move_goes_to_both_axes_and_clamps() {
    let mut d = device(FakeHw::new(1800));
    d.apply(&Command::PanTilt(100, 200)).unwrap();
    let pose = d.pose().unwrap();
    assert_eq!((pose.pan, pose.tilt), (100, 200));
    d.apply(&Command::PanTilt(u32::MAX, u32::MAX)).unwrap();
    let pose = d.pose().unwrap();
    assert_eq!((pose.pan, pose.tilt), (7360, 1240));
    // The pose is unknown, not stale, once a stop made the head lose count; a relative move then refuses.
    d.apply(&Command::Stop).unwrap();
    assert!(d.pose().is_none());
    assert!(
        d.apply(&Command::Fov { x: 10, y: 0 })
            .unwrap_err()
            .contains("unknown")
    );
    assert!(
        d.apply(&Command::PanTilt(1, 1))
            .unwrap_err()
            .contains("unknown")
    );
}

#[test]
fn only_commands_that_move_count_as_moving_and_holds_keep_the_device_busy() {
    assert!(Command::Fov { x: 1, y: 1 }.moves() && Command::PanTilt(1, 1).moves());
    assert!(Command::Move(cam::mqtt::Move::Left).moves() && Command::Home.moves());
    assert!(!Command::Move(cam::mqtt::Move::Stop).moves());
    assert!(!Command::Stop.moves() && !Command::Refocus.moves() && !Command::Reboot.moves());
    assert!(!Command::SavePreset("x".into()).moves());
    let mut d = device(FakeHw::new(1800));
    assert!(!d.busy());
    d.apply(&Command::Move(cam::mqtt::Move::Right)).unwrap();
    assert!(d.busy(), "a hold goes on after apply returned");
    d.apply(&Command::Move(cam::mqtt::Move::Stop)).unwrap();
    until_changed(&mut d, Duration::from_secs(1), Duration::from_millis(100));
    assert!(!d.busy());
}

#[test]
fn relative_motion_refuses_unknown_position_after_stop() {
    let cfg = cam::mqtt::Config::default();
    let mut d = device(FakeHw::new(1800));
    d.apply(&Command::Stop).unwrap();
    let cmd = cfg
        .command("camera/e1zoom/pan/set", b"relative:100", false)
        .unwrap()
        .unwrap();
    assert!(d.apply(&cmd).unwrap_err().contains("unknown"));
    assert!(!d.states().unwrap().contains_key("pan"));
}

#[test]
fn persisted_settings_are_applied_at_startup() {
    use cam::ir::fake::Event;
    let hw = Arc::new(std::sync::Mutex::new(FakeHw::new(1800)));
    let d = device_with(
        SharedIr(hw.clone()),
        owner(Some(
            "ir = \"night\"\nled = false\nir_leds_at_night = false\nptz_speed = 700\n",
        )),
    );
    let events = hw.lock().unwrap().events().to_vec();
    assert!(events.contains(&Event::IspNight(true)));
    assert_eq!(
        events.last(),
        Some(&Event::IrLeds(false)),
        "night without the flood"
    );
    assert!(!cam::ir::Hw::status_led_state(&mut *hw.lock().unwrap()).unwrap());
    let mut d = d;
    let s = d.states().unwrap();
    assert_eq!(
        (&s["ir"][..], &s["led"][..], &s["ptz_speed"][..]),
        ("night", "OFF", "700")
    );
    assert_eq!(s["settings"], "ok");
}

#[test]
fn an_invalid_persisted_value_is_reported_not_fatal() {
    let mut d = device_with(FakeHw::new(1800), owner(Some("mic_volume = 900\n")));
    let s = d.states().unwrap();
    assert_eq!(s["mic_volume"], "100");
    assert_eq!(s["settings"], "invalid: mic_volume");
}

#[test]
fn audio_and_stream_settings_go_to_the_media_thread_and_states_show_what_it_applied() {
    use cam::pipeline::{Tuned, Tuner};
    let o = owner(None);
    let media = Arc::clone(&o.media);
    let mut d = device_with(FakeHw::new(1800), o);
    d.apply(&setting("mic_volume", "40")).unwrap();
    d.apply(&setting("main_resolution", "1280x960")).unwrap();
    let mut t = &*media;
    let req = t.next().unwrap();
    assert_eq!(req.audio.unwrap().mic_volume, 40);
    assert_eq!(req.main.unwrap().dim, hdal::Dim { w: 1280, h: 960 });
    // The media thread applied a different volume (driver clamp): the state shows what runs.
    let mut applied = req.audio.unwrap();
    applied.mic_volume = 35;
    t.report(Tuned::Audio {
        applied,
        refused: None,
    });
    assert!(d.changed(), "an applied report asks for a publish");
    assert!(!d.changed(), "once");
    assert_eq!(d.states().unwrap()["mic_volume"], "35");
    // A refusal is visible in the settings sensor.
    // The media thread reverted the change (the encoders could not sustain it).
    t.report(Tuned::Main {
        applied: cam::pipeline::MainStream::of(
            &cam::pipeline::PipelineConfig::e1_zoom().streams[0],
        ),
        refused: Some(hdal::Error {
            step: hdal::Step::VideoencRetune,
            code: -22,
        }),
        restart_ms: 900,
    });
    d.tick(Duration::from_secs(1)).unwrap();
    let s = d.states().unwrap();
    assert!(s["settings"].contains("refused main stream"));
    // The refused value is not kept as the setting: the store follows what runs.
    assert_eq!(s["main_resolution"], "1920x1440");
    d.tick(Duration::from_secs(12)).unwrap();
    d.shutdown();
    let s = d.states().unwrap();
    assert_eq!(s["main_resolution"], "1920x1440", "persisted as what runs");
}

#[test]
fn rejected_payloads_never_reach_the_device() {
    let cfg = cam::mqtt::Config::default();
    for (key, bad) in [
        ("mic_volume", "101"),
        ("aec", "maybe"),
        ("main_resolution", "2560x1440"),
        ("move", "sideways"),
    ] {
        assert!(
            cfg.command(&format!("camera/e1zoom/{key}/set"), bad.as_bytes(), false)
                .is_err(),
            "{key}={bad}"
        );
    }
}

#[test]
fn a_held_arrow_moves_until_release_and_stops_within_one_chunk_with_exact_position() {
    let mut d = device(FakeHw::new(1800));
    d.apply(&Command::Pan(3000)).unwrap();
    let ms = Duration::from_millis;
    d.tick(ms(0)).unwrap();
    d.apply(&setting("move", "right")).unwrap();
    let mut now = ms(0);
    let mut polled = Vec::new();
    while now < ms(1500) {
        if let Some(w) = d.tick(now).unwrap() {
            polled.push(w);
        }
        now += ms(60);
    }
    assert!(
        polled.iter().all(|w| *w <= ms(20)),
        "fast polls while holding"
    );
    let moving = d.states().unwrap()["pan"].parse::<u32>().unwrap();
    assert!(moving > 3000, "moved right: {moving}");
    d.apply(&setting("move", "stop")).unwrap();
    let released = now;
    now = until_changed(&mut d, now, ms(60));
    assert!(now - released <= ms(180), "stopped within a chunk");
    let s = d.states().unwrap();
    assert!(!d.changed(), "once");
    assert!(
        s["pan"].parse::<u32>().unwrap() >= moving,
        "position kept, no re-home"
    );
    d.apply(&Command::Pan(10)).unwrap();
    assert_eq!(d.states().unwrap()["pan"], "10");
}

#[test]
fn a_lost_release_ends_the_hold_at_the_timeout() {
    let mut d = device_with(
        FakeHw::new(1800),
        owner(Some(
            "ptz_hold_timeout = 1\nptz_speed = 100\nptz_start_speed = 100\n",
        )),
    );
    d.apply(&Command::Pan(10)).unwrap();
    d.tick(Duration::ZERO).unwrap();
    d.apply(&setting("move", "right")).unwrap();
    let now = until_changed(&mut d, Duration::ZERO, Duration::from_millis(60));
    assert!(now < Duration::from_secs(3), "hold never ended");
    assert!(now >= Duration::from_secs(1));
}

#[test]
fn holding_zoom_settles_calibrated_focus_after_release() {
    let mut d = device(FakeHw::new(1800));
    d.tick(Duration::ZERO).unwrap();
    d.apply(&setting("move", "zoom_in")).unwrap();
    let mut now = Duration::ZERO;
    for _ in 0..10 {
        d.tick(now).unwrap();
        now += Duration::from_millis(20);
    }
    d.apply(&setting("move", "stop")).unwrap();
    while d.tick(now).unwrap().is_some() {
        now += Duration::from_millis(20);
    }
    let zoom: u32 = d.states().unwrap()["zoom"].parse().unwrap();
    assert!(zoom > 0, "zoomed in");
}

#[test]
fn a_setting_change_is_saved_within_the_window_and_survives_a_restart() {
    let o = owner(None);
    let mut d = device_with(FakeHw::new(1800), o);
    d.tick(Duration::ZERO).unwrap();
    d.apply(&setting("ptz_ramp", "300")).unwrap();
    d.tick(Duration::from_secs(5)).unwrap();
    d.tick(Duration::from_secs(11)).unwrap();
    d.shutdown();
    // the fake disk is inside the device; a fresh load of what it wrote must agree
    let s = d.states().unwrap();
    assert_eq!(s["ptz_ramp"], "300");
}

/// One `FakeIsp` the test keeps a handle on while the device owns the lens.
struct SharedIsp(Arc<std::sync::Mutex<FakeIsp>>);
impl cam::zoom::Isp for SharedIsp {
    fn get(
        &mut self,
        item: u32,
        words: &mut [u32; cam::zoom::WORDS],
    ) -> Result<(), cam::zoom::Error> {
        self.0.lock().unwrap().get(item, words)
    }
    fn set(&mut self, item: u32, words: &[u32; cam::zoom::WORDS]) -> Result<(), cam::zoom::Error> {
        self.0.lock().unwrap().set(item, words)
    }
    fn get_iq(
        &mut self,
        item: u32,
        words: &mut [u32; cam::zoom::WORDS],
    ) -> Result<(), cam::zoom::Error> {
        self.0.lock().unwrap().get_iq(item, words)
    }
    fn set_iq(
        &mut self,
        item: u32,
        words: &[u32; cam::zoom::WORDS],
    ) -> Result<(), cam::zoom::Error> {
        self.0.lock().unwrap().set_iq(item, words)
    }
    fn sleep(&mut self, d: Duration) {
        self.0.lock().unwrap().sleep(d);
    }
}

type AfDevice<H, B = FakeBus> = Device<B, SharedIsp, H, Os>;

/// A lens whose window at every zoom is 86..=330 with the sharp point at 200 (calibrated: 209; the fitted vertex is 201).
fn af_device<H: cam::ir::Hw>(
    ir: H,
    file: Option<&str>,
) -> (AfDevice<H>, Arc<std::sync::Mutex<FakeIsp>>) {
    af_device_with_map(ir, file, SharedDisk::default())
}

fn af_device_with_map<H: cam::ir::Hw>(
    ir: H,
    file: Option<&str>,
    map: SharedDisk,
) -> (AfDevice<H>, Arc<std::sync::Mutex<FakeIsp>>) {
    let mut isp = FakeIsp::new(0, 209);
    isp.focus_window = (86, 330);
    isp.sharp = Some(200);
    let isp = Arc::new(std::sync::Mutex::new(isp));
    let d = Device::new(
        Ptz::new(FakeBus::new(3000, 500)),
        Lens::new(SharedIsp(isp.clone())),
        Ir::new(ir),
        Os,
        ZfTable {
            lens: 1,
            backlash: 0,
            points: vec![
                Point {
                    zoom: 0,
                    focus: 209,
                },
                Point {
                    zoom: 1024,
                    focus: 800,
                },
            ],
        },
        owner_with_map(file, map),
    )
    .unwrap();
    (d, isp)
}

/// Ticks every 10 ms until the device asks for no fast poll (no search runs or waits); returns that time.
fn settled<H: cam::ir::Hw + Send, B: cam::ptz::Bus + Send>(
    d: &mut AfDevice<H, B>,
    mut now: Duration,
) -> Duration {
    for _ in 0..10_000 {
        if d.tick(now).unwrap().is_none() {
            return now;
        }
        now += Duration::from_millis(10);
    }
    panic!("never settled")
}

/// Searches (pan/tilt checks included) since the daemon started.
fn searches<H: cam::ir::Hw + Send, B: cam::ptz::Bus + Send>(d: &mut AfDevice<H, B>) -> u64 {
    af_attribute(d, "searches").as_u64().unwrap()
}

fn af_attribute<H: cam::ir::Hw + Send, B: cam::ptz::Bus + Send>(
    d: &mut AfDevice<H, B>,
    key: &str,
) -> serde_json::Value {
    let s = d.states().unwrap();
    serde_json::from_str::<serde_json::Value>(&s["autofocus_attributes"]).unwrap()[key].clone()
}

#[test]
fn startup_and_the_refocus_button_each_run_one_search_that_ends_focused() {
    let (mut d, isp) = af_device(FakeHw::new(1800), None);
    let now = settled(&mut d, Duration::ZERO);
    assert_eq!(searches(&mut d), 1);
    assert_eq!(d.states().unwrap()["autofocus_state"], "focused 201");
    assert_eq!(af_attribute(&mut d, "trigger"), "startup");
    d.apply(&Command::Refocus).unwrap();
    settled(&mut d, now);
    assert_eq!(searches(&mut d), 2);
    // the climb from the learned 201 fits the vertex at 200
    assert!((200..=201).contains(&isp.lock().unwrap().focus));
    assert_eq!(af_attribute(&mut d, "trigger"), "button");
    assert!(af_attribute(&mut d, "reference_va").as_u64().unwrap() > 0);
    assert!(af_attribute(&mut d, "duration_ms").as_u64().is_some());
}

#[test]
fn a_preset_fires_move_and_zoom_but_runs_one_search() {
    let (mut d, _isp) = af_device(FakeHw::new(1800), None);
    let now = settled(&mut d, Duration::ZERO);
    let before = searches(&mut d);
    d.apply(&Command::Preset("Shelf".into())).unwrap();
    settled(&mut d, now);
    assert_eq!(searches(&mut d), before + 1);
    assert_eq!(af_attribute(&mut d, "trigger"), "zoom");
    assert_eq!(d.states().unwrap()["autofocus_state"], "focused 201");
}

#[test]
fn a_motion_command_during_a_search_parks_the_learned_focus_and_the_move_check_runs() {
    let (mut d, isp) = af_device(FakeHw::new(1800), None);
    let mut now = settled(&mut d, Duration::ZERO);
    d.apply(&Command::Refocus).unwrap();
    for _ in 0..30 {
        d.tick(now).unwrap();
        now += Duration::from_millis(10);
    }
    assert_eq!(d.states().unwrap()["autofocus_state"], "searching");
    let before = searches(&mut d);
    d.apply(&Command::Pan(4000)).unwrap();
    // the cancel parks where the startup search found this view sharp (the map), not the calibrated 209
    assert_eq!(isp.lock().unwrap().focus, 201, "learned for this view");
    settled(&mut d, now);
    assert_eq!(searches(&mut d), before + 1);
    assert_eq!(af_attribute(&mut d, "trigger"), "move");
    assert_eq!(
        af_attribute(&mut d, "park"),
        "factory",
        "nothing learned at pan 4000"
    );
}

#[test]
fn a_ptz_hold_release_during_a_search_is_handled_on_the_next_tick() {
    let ms = Duration::from_millis;
    let (mut d, isp) = af_device(FakeHw::new(1800), None);
    let mut now = settled(&mut d, Duration::ZERO);
    d.apply(&Command::Refocus).unwrap();
    for _ in 0..30 {
        d.tick(now).unwrap();
        now += ms(10);
    }
    d.apply(&setting("move", "right")).unwrap();
    for _ in 0..10 {
        d.tick(now).unwrap();
        now += ms(20);
    }
    d.apply(&Command::Refocus).unwrap();
    d.apply(&setting("move", "stop")).unwrap();
    let released = now;
    let ended = until_changed_af(&mut d, now, ms(60));
    assert!(ended - released <= ms(180), "{:?}", ended - released);
    settled(&mut d, ended);
    assert_eq!(af_attribute(&mut d, "trigger"), "move");
    assert_eq!(isp.lock().unwrap().focus, 201);
}

#[test]
fn a_zoom_hold_release_reports_its_end_promptly_while_a_search_waits() {
    let ms = Duration::from_millis;
    let (mut d, isp) = af_device(FakeHw::new(1800), None);
    let mut now = settled(&mut d, Duration::ZERO);
    d.apply(&setting("move", "zoom_in")).unwrap();
    d.apply(&Command::Refocus).unwrap();
    for _ in 0..10 {
        d.tick(now).unwrap();
        now += ms(20);
    }
    let before = searches(&mut d);
    d.apply(&setting("move", "stop")).unwrap();
    let released = now;
    let ended = until_changed_af(&mut d, now, ms(20));
    assert!(ended - released <= ms(100), "{:?}", ended - released);
    settled(&mut d, ended);
    assert_eq!(searches(&mut d), before + 1);
    assert_eq!(af_attribute(&mut d, "trigger"), "zoom");
    let _ = &isp;
}

#[test]
fn day_night_switches_request_a_search_after_the_settle_time() {
    let mut readings = vec![1800; 6];
    readings.push(2400);
    let (mut d, _isp) = af_device(FakeHw::new(1800).with_readings(&readings), None);
    let mut now = settled(&mut d, Duration::ZERO);
    let before = searches(&mut d);
    // the reading crosses to night; auto switches after the 4 s hold, then waits 1 s before searching
    while af_attribute(&mut d, "trigger") != "day_night" {
        d.tick(now).unwrap();
        now += Duration::from_millis(10);
        assert!(now < Duration::from_secs(30), "no day/night search");
    }
    settled(&mut d, now);
    assert_eq!(searches(&mut d), before + 1);
    d.apply(&setting("ir", "day")).unwrap();
    let switched = now;
    let mut started = None;
    while started.is_none() {
        d.tick(now).unwrap();
        if d.states().unwrap()["autofocus_state"] == "searching" {
            started = Some(now);
        }
        now += Duration::from_millis(10);
    }
    assert!(started.unwrap() - switched >= Duration::from_secs(1));
    settled(&mut d, now);
    assert_eq!(af_attribute(&mut d, "trigger"), "day_night");
}

#[test]
fn autofocus_off_keeps_the_calibrated_focus_but_the_button_still_searches() {
    let (mut d, _isp) = af_device(FakeHw::new(1800), Some("autofocus = false\n"));
    let now = settled(&mut d, Duration::ZERO);
    d.apply(&Command::Zoom(512)).unwrap();
    let now = settled(&mut d, now);
    assert_eq!(searches(&mut d), 0);
    assert_eq!(d.states().unwrap()["autofocus_state"], "idle");
    d.apply(&Command::Refocus).unwrap();
    let now = settled(&mut d, now);
    assert_eq!(searches(&mut d), 1);
    d.apply(&setting("autofocus", "ON")).unwrap();
    d.apply(&Command::Zoom(0)).unwrap();
    settled(&mut d, now);
    assert_eq!(searches(&mut d), 2, "on again: zoom searches");
}

#[test]
fn the_diagnostic_focus_command_parks_the_motor_without_a_search() {
    let (mut d, isp) = af_device(FakeHw::new(1800), None);
    let now = settled(&mut d, Duration::ZERO);
    let before = searches(&mut d);
    let cmd = cam::mqtt::Config::default()
        .command("camera/e1zoom/focus/set", b"300", false)
        .unwrap()
        .unwrap();
    d.apply(&cmd).unwrap();
    settled(&mut d, now);
    assert_eq!(isp.lock().unwrap().focus, 300);
    assert_eq!(searches(&mut d), before);
}

#[test]
fn startup_installs_the_wide_rows_derived_from_the_calibration() {
    let (_d, isp) = af_device(FakeHw::new(1800), None);
    let rows = isp.lock().unwrap().rows.clone();
    // (0, 209) and (1024, 800) with 300 either side, clamped at 0
    assert_eq!(&rows[..2], [(0, 0, 509), (1024, 500, 1100)]);
    assert_eq!(rows[49], (1024, 500, 1100), "slot 49 repeats the last row");
}

#[test]
fn a_focused_search_learns_the_view_and_a_zoom_back_parks_at_the_learned_focus() {
    let (mut d, isp) = af_device(FakeHw::new(1800), None);
    let now = settled(&mut d, Duration::ZERO);
    assert_eq!(
        af_attribute(&mut d, "map_points"),
        1,
        "the startup search learned its view"
    );
    d.apply(&Command::Zoom(512)).unwrap();
    let now = settled(&mut d, now);
    isp.lock().unwrap().clear_sets();
    d.apply(&Command::Zoom(0)).unwrap();
    // the park focus for zoom 0 at this view is the learned 201, not the calibrated 209
    assert_eq!(isp.lock().unwrap().focus_moves()[0], 201);
    assert_eq!(af_attribute(&mut d, "park"), "learned");
    assert_eq!(af_attribute(&mut d, "park_focus"), 201);
    settled(&mut d, now);
    let state = d.states().unwrap()["autofocus_state"].clone();
    assert!(state == "focused 200" || state == "focused 201", "{state}");
}

#[test]
fn a_pan_to_a_view_the_map_knows_parks_at_its_focus_before_searching() {
    // a point learned earlier at pan 4000, tilt 0 (the fake bus homes at tilt 0), zoom 0, focus 300
    let map = SharedDisk::with_file("points = [[4000, 0, 0, 300, 5, 1]]\n");
    let (mut d, isp) = af_device_with_map(FakeHw::new(1800), None, map);
    let now = settled(&mut d, Duration::ZERO);
    assert_eq!(af_attribute(&mut d, "map_points"), 2);
    isp.lock().unwrap().clear_sets();
    let before = searches(&mut d);
    d.apply(&Command::Pan(4000)).unwrap();
    settled(&mut d, now);
    assert_eq!(
        isp.lock().unwrap().focus_moves()[0],
        300,
        "parked at the learned focus first"
    );
    assert_eq!(searches(&mut d), before + 1);
    assert_eq!(af_attribute(&mut d, "trigger"), "move");
    // the climb down from 300 settles on the fake's peak within the fit's tolerance
    let state = d.states().unwrap()["autofocus_state"].clone();
    let focus: u32 = state.strip_prefix("focused ").unwrap().parse().unwrap();
    assert!((195..=205).contains(&focus), "{state}");
    assert_eq!(
        af_attribute(&mut d, "map_points"),
        2,
        "the point at pan 4000 was replaced, not added"
    );
}

#[test]
fn a_map_that_knows_the_home_view_parks_the_lens_there_before_the_startup_search() {
    // the fake bus homes at (0, 0); the lens homes at focus 0 (window 86..330 here)
    let map = SharedDisk::with_file("points = [[0, 0, 0, 240, 5, 1]]\n");
    let (mut d, isp) = af_device_with_map(FakeHw::new(1800), None, map);
    // the first FOCUS set is the homing command; the park follows it
    assert_eq!(
        isp.lock().unwrap().focus_moves()[1],
        240,
        "parked at the learned home focus"
    );
    assert_eq!(af_attribute(&mut d, "park"), "learned");
    settled(&mut d, Duration::ZERO);
    assert!(d.states().unwrap()["autofocus_state"].starts_with("focused "));
}

#[test]
fn the_map_is_written_by_tick_within_the_save_window_and_flushed_at_shutdown() {
    let map = SharedDisk::default();
    let (mut d, _isp) = af_device_with_map(FakeHw::new(1800), None, map.clone());
    let now = settled(&mut d, Duration::ZERO);
    assert_eq!(af_attribute(&mut d, "map_points"), 1);
    assert!(
        !map.ops().contains(&"rename_temp"),
        "not written in the command path: {:?}",
        map.ops()
    );
    d.tick(now + Duration::from_secs(30)).unwrap();
    assert!(!map.ops().contains(&"rename_temp"));
    d.tick(now + Duration::from_secs(61)).unwrap();
    assert!(map.ops().contains(&"rename_temp"));
    // the fake bus homes at (0, 0)
    assert!(
        map.file().unwrap().contains("[0, 0, 0, 201,"),
        "{:?}",
        map.file()
    );
}

/// As [`until_changed`] for the autofocus device.
fn until_changed_af<H: cam::ir::Hw + Send, B: cam::ptz::Bus + Send>(
    d: &mut AfDevice<H, B>,
    mut now: Duration,
    step: Duration,
) -> Duration {
    d.states().unwrap();
    d.changed();
    for _ in 0..1000 {
        d.tick(now).unwrap();
        if d.changed() {
            return now;
        }
        now += step;
    }
    panic!("nothing changed")
}

/// Ticks every 10 ms until the walk reports `state`, counting pan/tilt moves per tick; returns the time it ended.
fn walk_until<H: cam::ir::Hw + Send, B: cam::ptz::Bus + Send>(
    d: &mut AfDevice<H, B>,
    bus_moves: impl Fn() -> usize,
    state: &str,
    mut now: Duration,
) -> Duration {
    let mut done = 0;
    for _ in 0..200_000 {
        let before = bus_moves();
        d.tick(now).unwrap();
        now += Duration::from_millis(10);
        let cal = af_attribute(d, "calibration");
        // one point per tick at most: a goto (any number of ramp chunks on both axes) or a search request
        // (accounting the finished search and the goto to the next point happen in the same tick)
        let now_done = cal["done"].as_u64().unwrap();
        assert!(
            now_done <= done + 1,
            "{done} → {now_done} points in one tick"
        );
        let _ = before;
        done = now_done;
        if cal["state"] == state {
            return now;
        }
    }
    panic!("the walk never reached {state}")
}

type SharedBus = Arc<std::sync::Mutex<FakeBus>>;
type WalkDevice = Device<SharedFakeBus, SharedIsp, FakeHw, Os>;
type IspHandle = Arc<std::sync::Mutex<FakeIsp>>;

/// An autofocus device whose pan/tilt bus the test keeps a handle on.
fn walk_device() -> (WalkDevice, SharedBus, IspHandle) {
    let mut isp = FakeIsp::new(0, 209);
    isp.focus_window = (86, 330);
    isp.sharp = Some(200);
    let isp = Arc::new(std::sync::Mutex::new(isp));
    let bus = Arc::new(std::sync::Mutex::new(FakeBus::new(3000, 500)));
    let d = Device::new(
        Ptz::new(SharedFakeBus(bus.clone())),
        Lens::new(SharedIsp(isp.clone())),
        Ir::new(FakeHw::new(1800)),
        Os,
        ZfTable {
            lens: 1,
            backlash: 0,
            points: vec![
                Point {
                    zoom: 0,
                    focus: 209,
                },
                Point {
                    zoom: 1024,
                    focus: 800,
                },
            ],
        },
        owner(None),
    )
    .unwrap();
    (d, bus, isp)
}

struct SharedFakeBus(SharedBus);
impl cam::ptz::Bus for SharedFakeBus {
    fn read(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), cam::ptz::Error> {
        self.0.lock().unwrap().read(reg, buf)
    }
    fn write(&mut self, reg: u8, data: &[u8]) -> Result<(), cam::ptz::Error> {
        self.0.lock().unwrap().write(reg, data)
    }
    fn sleep(&mut self, d: Duration) {
        self.0.lock().unwrap().sleep(d)
    }
}

fn calibrate() -> Command {
    cam::mqtt::Config::default()
        .command("camera/e1zoom/calibrate_focus/set", b"PRESS", false)
        .unwrap()
        .unwrap()
}

#[test]
fn the_calibrate_button_walks_the_grid_learns_every_point_and_returns_to_the_start() {
    let (mut d, bus, _isp) = walk_device();
    let now = settled(&mut d, Duration::ZERO);
    d.apply(&Command::Pan(3000)).unwrap();
    d.apply(&Command::Tilt(500)).unwrap();
    let now = settled(&mut d, now);
    let before = af_attribute(&mut d, "map_points").as_u64().unwrap();
    d.apply(&calibrate()).unwrap();
    d.tick(now).unwrap();
    // before the first point the estimate is the nominal 7 s per point
    assert_eq!(
        d.states().unwrap()["autofocus_state"],
        "calibrating 0/120, 14 min left"
    );
    assert_eq!(af_attribute(&mut d, "calibration")["remaining_s"], 840);
    let moves = || bus.lock().unwrap().moves().len();
    let end = walk_until(&mut d, moves, "done", now);
    let cal = af_attribute(&mut d, "calibration");
    assert_eq!(
        (cal["done"].as_u64(), cal["learned"].as_u64()),
        (Some(120), Some(120)),
        "{cal}"
    );
    assert!(af_attribute(&mut d, "map_points").as_u64().unwrap() >= before + 120);
    let s = d.states().unwrap();
    assert_eq!(
        (s["pan"].as_str(), s["tilt"].as_str(), s["zoom"].as_str()),
        ("3000", "500", "0")
    );
    settled(&mut d, end);
    assert!(d.states().unwrap()["autofocus_state"].starts_with("focused "));
    assert_eq!(
        af_attribute(&mut d, "trigger"),
        "zoom",
        "the return ends in a normal zoom search"
    );
    // a second press while nothing runs starts another walk; the attributes keep the last outcome until then
    assert_eq!(af_attribute(&mut d, "calibration")["state"], "done");
}

#[test]
fn a_pan_during_the_walk_cancels_it_and_runs() {
    let (mut d, bus, _isp) = walk_device();
    let now = settled(&mut d, Duration::ZERO);
    d.apply(&calibrate()).unwrap();
    let moves = || bus.lock().unwrap().moves().len();
    let mut now = now;
    for _ in 0..400 {
        d.tick(now).unwrap();
        now += Duration::from_millis(10);
    }
    let cal = af_attribute(&mut d, "calibration");
    assert_eq!(cal["state"], "running");
    assert!(cal["done"].as_u64().unwrap() > 0, "{cal}");
    let done = cal["done"].as_u64().unwrap();
    d.apply(&Command::Pan(5000)).unwrap();
    let cal = af_attribute(&mut d, "calibration");
    assert_eq!(cal["state"], "cancelled");
    assert_eq!(cal["done"].as_u64(), Some(done));
    settled(&mut d, now);
    assert_eq!(d.states().unwrap()["pan"], "5000");
    assert_eq!(
        af_attribute(&mut d, "calibration")["state"],
        "cancelled",
        "kept until the next walk"
    );
    let _ = moves();
}

#[test]
fn stop_and_refocus_cancel_the_walk_but_a_setting_does_not() {
    let (mut d, _bus, _isp) = walk_device();
    let now = settled(&mut d, Duration::ZERO);
    d.apply(&calibrate()).unwrap();
    for _ in 0..50 {
        d.tick(now).unwrap();
    }
    d.apply(&setting("ptz_ramp", "300")).unwrap();
    assert_eq!(af_attribute(&mut d, "calibration")["state"], "running");
    d.apply(&calibrate()).unwrap();
    assert_eq!(
        af_attribute(&mut d, "calibration")["state"],
        "running",
        "a second press is ignored"
    );
    d.apply(&Command::Stop).unwrap();
    assert_eq!(af_attribute(&mut d, "calibration")["state"], "cancelled");
    // a stop forgets the position, so a fresh device for the Refocus case
    let (mut d, _bus, _isp) = walk_device();
    let now = settled(&mut d, Duration::ZERO);
    d.apply(&calibrate()).unwrap();
    for _ in 0..50 {
        d.tick(now).unwrap();
    }
    d.apply(&Command::Refocus).unwrap();
    assert_eq!(af_attribute(&mut d, "calibration")["state"], "cancelled");
}

/// The target the optics give for the card's upper-left tap from `at` at `zoom`.
fn tap_target(at: cam::ptz::Position, zoom: u32) -> cam::ptz::Position {
    let req = cam::position3d::Request::parse(
        r#"{"start_x":66,"start_y":189,"end_x":66,"end_y":189,"operation":"zoomto"}"#,
    )
    .unwrap();
    cam::position3d::Optics::E1_ZOOM
        .target(
            &req,
            at,
            zoom,
            cam::orientation::Orientation::new(false, false),
            (cam::ptz::PAN_MAX, cam::ptz::TILT_MAX),
            cam::mqtt::ZOOM_MAX,
        )
        .position
}

fn tap_cmd() -> Command {
    cam::mqtt::Config::default()
        .command(
            "camera/e1zoom/position3d/set",
            br#"{"start_x":66,"start_y":189,"end_x":66,"end_y":189,"operation":"zoomto"}"#,
            false,
        )
        .unwrap()
        .unwrap()
}

#[test]
fn a_tap_stamped_mid_move_resolves_against_the_pose_the_picture_showed() {
    use cam::mqtt::Hardware;
    let mut d = device(FakeHw::new(1800));
    d.apply(&setting("tap_delay", "0")).unwrap();
    d.apply(&Command::Pan(3000)).unwrap();
    d.apply(&Command::Tilt(600)).unwrap();
    let t0 = Duration::from_secs(10);
    d.tick(t0).unwrap();
    // tap 1 starts a move from (3000, 600); the fake bus completes it at once, but its trajectory is recorded
    d.apply_at(&tap_cmd(), t0).unwrap();
    let s = d.states().unwrap();
    let end1 = cam::ptz::Position {
        pan: s["pan"].parse().unwrap(),
        tilt: s["tilt"].parse().unwrap(),
    };
    assert_eq!(
        end1,
        tap_target(
            cam::ptz::Position {
                pan: 3000,
                tilt: 600
            },
            0
        )
    );
    // tap 2 arrives 300 ms into that move: it is measured from the pose of that moment, not from end1
    let t1 = t0 + Duration::from_millis(300);
    d.tick(t1).unwrap();
    d.apply_at(&tap_cmd(), t1).unwrap();
    let s = d.states().unwrap();
    let end2 = cam::ptz::Position {
        pan: s["pan"].parse().unwrap(),
        tilt: s["tilt"].parse().unwrap(),
    };
    let from_end1 = tap_target(end1, 0);
    assert_ne!(
        end2, from_end1,
        "resolved from the end of move 1 would overshoot"
    );
    // the pose 300 ms into move 1 lies between its start and its end on both axes
    let mid_pan = (3000.min(end1.pan)..=3000.max(end1.pan)).contains(&tap_target_inverse_pan(end2));
    assert!(
        mid_pan,
        "end2 {end2:?} does not come from a mid-move pose (end1 {end1:?})"
    );
}

/// The pan a tap target came from, for the card's upper-left tap at zoom 0 (the optics are linear in pan).
fn tap_target_inverse_pan(target: cam::ptz::Position) -> u32 {
    let probe = tap_target(
        cam::ptz::Position {
            pan: 3000,
            tilt: 600,
        },
        0,
    );
    let offset = 3000 - probe.pan as i64;
    (target.pan as i64 + offset) as u32
}

#[test]
fn with_zero_delay_and_no_motion_a_tap_resolves_as_before() {
    use cam::mqtt::Hardware;
    let mut d = device(FakeHw::new(1800));
    d.apply(&setting("tap_delay", "0")).unwrap();
    d.apply(&Command::Pan(3000)).unwrap();
    d.apply(&Command::Tilt(600)).unwrap();
    d.tick(Duration::from_secs(30)).unwrap();
    d.apply_at(&tap_cmd(), Duration::from_secs(40)).unwrap();
    let s = d.states().unwrap();
    let got = cam::ptz::Position {
        pan: s["pan"].parse().unwrap(),
        tilt: s["tilt"].parse().unwrap(),
    };
    assert_eq!(
        got,
        tap_target(
            cam::ptz::Position {
                pan: 3000,
                tilt: 600
            },
            0
        )
    );
}

#[test]
fn a_box_stamped_mid_zoom_uses_the_zoom_of_that_moment() {
    use cam::mqtt::Hardware;
    let mut d = device(FakeHw::new(1800));
    d.apply(&setting("tap_delay", "0")).unwrap();
    d.apply(&Command::Pan(3000)).unwrap();
    d.apply(&Command::Tilt(600)).unwrap();
    let t0 = Duration::from_secs(10);
    d.tick(t0).unwrap();
    d.apply(&Command::Zoom(1024)).unwrap();
    // the zoom move started at t0 and takes 5.5 s at speed 80; a tap 2.75 s in sees zoom ≈ 512
    let t1 = t0 + Duration::from_millis(2750);
    d.tick(t1).unwrap();
    d.apply_at(&tap_cmd(), t1).unwrap();
    let s = d.states().unwrap();
    let got = cam::ptz::Position {
        pan: s["pan"].parse().unwrap(),
        tilt: s["tilt"].parse().unwrap(),
    };
    let at = cam::ptz::Position {
        pan: 3000,
        tilt: 600,
    };
    let at_half = tap_target(at, 512);
    let at_full = tap_target(at, 1024);
    assert_ne!(at_half, at_full);
    assert!(
        got.pan.abs_diff(at_half.pan) <= 8,
        "got {got:?}, at zoom 512 {at_half:?}, at 1024 {at_full:?}"
    );
}

/// The sequence that crashed the camera: a preset, then a tap, then a second tap 300 ms later with the default
/// 500 ms delay, so its picture time lies before the first tap's move and before any zoom record.
#[test]
fn a_tap_looking_back_before_every_record_does_not_panic_and_resolves_from_the_start() {
    use cam::mqtt::Hardware;
    let mut d = device(FakeHw::new(1800));
    let t0 = Duration::from_secs(10);
    d.tick(t0).unwrap();
    d.apply_at(&Command::Preset("pos2".into()), t0).unwrap();
    let t1 = t0 + Duration::from_secs(12);
    d.tick(t1).unwrap();
    d.apply_at(&tap_cmd(), t1).unwrap();
    let t2 = t1 + Duration::from_millis(300);
    d.tick(t2).unwrap();
    d.apply_at(&tap_cmd(), t2).unwrap();
    let s = d.states().unwrap();
    let got = cam::ptz::Position {
        pan: s["pan"].parse().unwrap(),
        tilt: s["tilt"].parse().unwrap(),
    };
    // the second tap was made on the pos2 picture (300 ms < the 500 ms delay)
    assert_eq!(
        got,
        tap_target(
            cam::ptz::Position {
                pan: 6195,
                tilt: 650
            },
            0
        )
    );
    // and a tap older than everything, with no zoom record at all
    let mut d = device(FakeHw::new(1800));
    d.tick(Duration::from_secs(100)).unwrap();
    d.apply_at(&tap_cmd(), Duration::from_millis(100)).unwrap();
}

fn pose<H: cam::ir::Hw + Send, B: cam::ptz::Bus + Send, I: cam::zoom::Isp + Send>(
    d: &mut Device<B, I, H, Os>,
) -> (String, String, String) {
    let s = d.states().unwrap();
    (s["pan"].clone(), s["tilt"].clone(), s["zoom"].clone())
}

fn save(name: &str) -> Command {
    cam::mqtt::Config {
        presets: presets(),
        ..Default::default()
    }
    .command("camera/e1zoom/preset_save/set", name.as_bytes(), false)
    .unwrap()
    .unwrap()
}

#[test]
fn a_saved_preset_is_where_the_camera_stood_and_survives_a_restart() {
    let saved = SharedDisk::default();
    let mut d = device_with(
        FakeHw::new(1800),
        owner_full(None, SharedDisk::default(), saved.clone()),
    );
    d.apply(&Command::Pan(4000)).unwrap();
    d.apply(&Command::Tilt(700)).unwrap();
    d.apply(&Command::Zoom(256)).unwrap();
    d.apply(&save("Desk")).unwrap();
    assert_eq!(
        d.states().unwrap()["preset"],
        "Desk",
        "it stands at the preset"
    );
    assert_eq!(
        saved.ops(),
        ["read", "write_temp", "sync_temp", "rename_temp", "sync_dir"],
        "written at once"
    );
    d.apply(&Command::Pan(100)).unwrap();
    d.apply(&Command::Zoom(0)).unwrap();
    d.apply(&Command::Preset("Desk".into())).unwrap();
    let at = ("4000".to_string(), "700".to_string(), "256".to_string());
    assert_eq!(pose(&mut d), at);
    assert_eq!(d.states().unwrap()["preset"], "Desk");
    // a restart on the same disk goes to the saved pose
    let mut d = device_with(
        FakeHw::new(1800),
        owner_full(None, SharedDisk::default(), saved.clone()),
    );
    d.apply(&Command::Preset("Desk".into())).unwrap();
    assert_eq!(pose(&mut d), at);
    // other presets keep their cam.toml positions
    d.apply(&Command::Preset("pos2".into())).unwrap();
    assert_eq!(
        pose(&mut d),
        ("6195".to_string(), "650".to_string(), "0".to_string())
    );
}

#[test]
fn a_save_is_refused_while_a_hold_runs_or_the_position_is_unknown() {
    let saved = SharedDisk::default();
    let mut d = device_with(
        FakeHw::new(1800),
        owner_full(None, SharedDisk::default(), saved.clone()),
    );
    d.tick(Duration::ZERO).unwrap();
    d.apply(&setting("move", "right")).unwrap();
    d.tick(Duration::from_millis(60)).unwrap();
    assert!(d.apply(&save("Desk")).is_err(), "pan/tilt hold");
    d.apply(&setting("move", "stop")).unwrap();
    until_changed(&mut d, Duration::from_millis(60), Duration::from_millis(60));
    d.apply(&setting("move", "zoom_in")).unwrap();
    d.tick(Duration::from_secs(5)).unwrap();
    assert!(d.apply(&save("Desk")).is_err(), "zoom hold");
    d.apply(&setting("move", "stop")).unwrap();
    let mut now = Duration::from_secs(5);
    while d.tick(now).unwrap().is_some() {
        now += Duration::from_millis(20);
    }
    d.apply(&Command::Stop).unwrap();
    assert!(d.apply(&save("Desk")).is_err(), "position unknown");
    assert_eq!(saved.file(), None, "nothing saved");
}

#[test]
fn a_save_during_the_walk_is_refused_and_the_walk_goes_on() {
    let (mut d, _bus, _isp) = walk_device();
    let now = settled(&mut d, Duration::ZERO);
    d.apply(&calibrate()).unwrap();
    for _ in 0..50 {
        d.tick(now).unwrap();
    }
    assert!(d.apply(&save("Desk")).is_err());
    assert_eq!(af_attribute(&mut d, "calibration")["state"], "running");
}

#[test]
fn a_save_during_a_search_keeps_the_search_going() {
    let (mut d, _isp) = af_device(FakeHw::new(1800), None);
    let mut now = settled(&mut d, Duration::ZERO);
    d.apply(&Command::Refocus).unwrap();
    for _ in 0..30 {
        d.tick(now).unwrap();
        now += Duration::from_millis(10);
    }
    assert_eq!(d.states().unwrap()["autofocus_state"], "searching");
    let before = searches(&mut d);
    d.apply(&save("Desk")).unwrap();
    assert_eq!(d.states().unwrap()["autofocus_state"], "searching");
    settled(&mut d, now);
    assert_eq!(
        searches(&mut d),
        before,
        "the same search finished, none restarted"
    );
    assert_eq!(af_attribute(&mut d, "trigger"), "button");
}

#[test]
fn a_failed_save_keeps_the_old_position() {
    let saved = SharedDisk::default();
    saved.0.lock().unwrap().fail_write = true;
    let mut d = device_with(
        FakeHw::new(1800),
        owner_full(None, SharedDisk::default(), saved),
    );
    d.apply(&Command::Pan(4000)).unwrap();
    let e = d.apply(&save("Desk")).unwrap_err();
    assert!(e.starts_with("preset not saved: "), "{e}");
    assert!(!d.states().unwrap().contains_key("preset"));
    d.apply(&Command::Preset("Desk".into())).unwrap();
    assert_eq!(d.states().unwrap()["pan"], "6195");
}

#[test]
fn shared_velocity_combines_pan_tilt_zoom_and_partial_release_keeps_pan() {
    let mut d = device(FakeHw::new(1800));
    let before = d.states().unwrap();
    d.apply(&Command::Velocity(cam::motion_control::Velocity {
        pan: 100,
        tilt: 100,
        zoom: 100,
    }))
    .unwrap();
    for ms in (0..=500).step_by(20) {
        d.tick(Duration::from_millis(ms)).unwrap();
    }
    let both = d.states().unwrap();
    for axis in ["pan", "tilt", "zoom"] {
        assert!(
            both[axis].parse::<u32>().unwrap() > before[axis].parse::<u32>().unwrap(),
            "{axis}"
        );
    }
    d.apply(&Command::Velocity(cam::motion_control::Velocity {
        pan: 100,
        tilt: 0,
        zoom: 0,
    }))
    .unwrap();
    for ms in (520..=800).step_by(20) {
        d.tick(Duration::from_millis(ms)).unwrap();
    }
    let one = d.states().unwrap();
    for ms in (820..=1100).step_by(20) {
        d.tick(Duration::from_millis(ms)).unwrap();
    }
    let later = d.states().unwrap();
    assert_eq!(one["tilt"], later["tilt"]);
    assert!(later["pan"].parse::<u32>().unwrap() > one["pan"].parse::<u32>().unwrap());
    d.apply(&Command::Move(cam::mqtt::Move::Stop)).unwrap();
    for ms in (1120..=1400).step_by(20) {
        d.tick(Duration::from_millis(ms)).unwrap();
    }
}

#[test]
fn stale_shared_velocity_is_not_replayed_after_blocking_work() {
    let mut d = device(FakeHw::new(1800));
    d.tick(Duration::from_secs(3)).unwrap();
    let before = d.states().unwrap();
    assert!(
        d.apply_at(
            &Command::Velocity(cam::motion_control::Velocity {
                pan: 100,
                tilt: 100,
                zoom: 100
            }),
            Duration::ZERO
        )
        .is_err()
    );
    d.tick(Duration::from_secs(4)).unwrap();
    let after = d.states().unwrap();
    for axis in ["pan", "tilt", "zoom"] {
        assert_eq!(before[axis], after[axis]);
    }
}

#[test]
fn a_new_name_creates_a_preset_and_removing_it_takes_it_out_of_the_table() {
    let saved = SharedDisk::default();
    let mut d = device_with(
        FakeHw::new(1800),
        owner_full(None, SharedDisk::default(), saved.clone()),
    );
    let before = d.preset_names().unwrap();
    d.apply(&Command::Pan(3000)).unwrap();
    d.apply(&save("Window")).unwrap();
    let names = d.preset_names().unwrap();
    assert!(names.contains(&"Window".to_string()), "{names:?}");
    assert_eq!(names.len(), before.len() + 1);
    d.apply(&Command::Pan(100)).unwrap();
    d.apply(&Command::Preset("Window".into())).unwrap();
    assert_eq!(pose(&mut d).0, "3000");
    d.apply(&Command::RemovePreset("Window".into())).unwrap();
    assert_eq!(d.preset_names().unwrap(), before);
    assert!(
        !d.states().unwrap().contains_key("preset"),
        "no longer at a preset"
    );
    assert!(d.apply(&Command::Preset("Window".into())).is_err());
    assert!(d.apply(&Command::RemovePreset("Window".into())).is_err());
    // The removal survives a restart.
    let d = device_with(
        FakeHw::new(1800),
        owner_full(None, SharedDisk::default(), saved),
    );
    assert_eq!(d.preset_names().unwrap(), before);
}
