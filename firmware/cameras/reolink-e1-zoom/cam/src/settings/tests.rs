//! Schema, loading, validation and the save window, against `FakeDisk`.
use super::*;

const STOCK: &str =
    r#"<isp_config_t ledstate="0"><channel brightness="128" mirror="1" flip="0" /></isp_config_t>"#;

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

fn store(file: Option<&str>) -> (Store<FakeDisk>, Vec<Problem>) {
    let disk = FakeDisk {
        file: file.map(str::to_owned),
        ..FakeDisk::default()
    };
    Store::load(disk, &StockDefaults::from_vendor(STOCK))
}

#[test]
fn keys_are_unique_and_every_default_is_valid() {
    let mut keys: Vec<_> = specs().iter().map(|s| s.spec.key).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), specs().len());
    for (slot, s) in specs().iter().enumerate() {
        assert!(s.spec.valid(s.spec.default), "{}", s.spec.key);
        assert_eq!(usize::from(s.spec.slot), slot, "a slot is its place");
        assert_eq!(s.spec.section, s.section().map(Section::slug));
    }
    shared::check_catalog(&SPECS).unwrap();
}

#[test]
fn absent_file_gives_the_first_run_defaults_alex_chose() {
    let (s, problems) = store(None);
    assert!(problems.is_empty());
    let a = s.settings().audio();
    assert_eq!((a.mic_volume, a.speaker_volume), (100, 100));
    assert!(!a.aec.enabled && !a.anr.enabled, "AEC and ANR start off");
    assert_eq!(a.aec.noise_level, -60);
    let m = s.settings().main_stream();
    assert_eq!(
        (m.dim.w, m.dim.h, m.bitrate_bps, m.fps),
        (1920, 1440, 1_024_000, 15)
    );
    assert_eq!(s.settings().ir_mode(), "auto");
    assert!(s.settings().led());
    let (p, hold) = s.settings().motion();
    assert_eq!(
        (p.start_pps, p.max_pps, p.ramp),
        (300, ptz::MAX_PPS, Duration::from_millis(1500))
    );
    assert_eq!(hold, secs(20));
}

#[test]
fn orientation_is_migrated_from_the_stock_file() {
    let (s, _) = store(None);
    assert_eq!(s.settings().get("mirror"), Some(1));
    assert_eq!(s.settings().get("flip"), Some(0));
    assert_eq!(
        s.settings().orientation(),
        crate::orientation::Orientation::new(true, false)
    );
}

#[test]
fn a_saved_file_is_read_back_and_unknown_keys_are_ignored() {
    let (s, problems) = store(Some(
        "mic_volume = 55\naec = true\nmain_resolution = \"2592x1944\"\nfuture_key = 3\n",
    ));
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(s.settings().audio().mic_volume, 55);
    assert!(s.settings().audio().aec.enabled);
    assert_eq!(s.settings().main_stream().dim.w, 2592);
}

#[test]
fn an_invalid_field_falls_back_alone_and_is_reported() {
    let (s, problems) = store(Some(
        "mic_volume = 500\nspeaker_volume = 40\nled = \"yes\"\n",
    ));
    assert_eq!(s.settings().audio().mic_volume, 100);
    assert_eq!(s.settings().audio().speaker_volume, 40);
    assert!(s.settings().led());
    let keys: Vec<_> = problems.iter().filter_map(|p| p.key.as_deref()).collect();
    assert_eq!(keys, ["mic_volume", "led"]);
    assert_eq!(summary(&problems), "invalid: mic_volume, led");
}

#[test]
fn an_unparsable_or_unreadable_file_gives_defaults_never_a_failure() {
    let (s, problems) = store(Some("this is = = not toml"));
    assert_eq!(
        s.settings(),
        &Settings::defaults(&StockDefaults::from_vendor(STOCK))
    );
    assert_eq!(problems.len(), 1);
    let disk = FakeDisk {
        fail_read: true,
        ..FakeDisk::default()
    };
    let (_, problems) = Store::load(disk, &StockDefaults::default());
    assert_eq!(summary(&problems), "invalid: file");
}

#[test]
fn inconsistent_ir_thresholds_in_the_file_both_fall_back() {
    let (s, problems) = store(Some("ir_to_night = 1000\nir_to_day = 2000\n"));
    let (t, _, _) = s.settings().ir();
    assert_eq!((t.to_night, t.to_day), (2300, 1900));
    assert_eq!(problems.len(), 2);
}

#[test]
fn mqtt_payloads_are_validated_against_the_table() {
    let (mut s, _) = store(None);
    assert!(s.set("mic_volume", "101", secs(0)).is_err());
    assert!(s.set("mic_volume", "abc", secs(0)).is_err());
    assert!(s.set("aec", "on", secs(0)).is_err());
    assert!(
        s.set("main_resolution", "2560x1440", secs(0)).is_err(),
        "no 16:9 crop"
    );
    assert!(s.set("nope", "1", secs(0)).is_err());
    assert!(!s.dirty(), "rejections change nothing");
    let c = s.set("mic_volume", "40.0", secs(0)).unwrap().unwrap();
    assert_eq!((c.spec.spec.key, c.value), ("mic_volume", 40));
    assert_eq!(s.set("mic_volume", "40", secs(1)).unwrap(), None, "no-op");
    assert!(
        s.set("ir_to_day", "2400", secs(0)).is_err(),
        "must stay below to_night"
    );
    assert!(s.set("ptz_start_speed", "700", secs(0)).is_ok());
    assert!(
        s.set("ptz_speed", "721", secs(0)).is_err(),
        "above the measured maximum"
    );
    assert!(
        s.set("ptz_speed", "600", secs(0)).is_err(),
        "below the start speed"
    );
}

#[test]
fn every_main_resolution_is_full_frame_4_3() {
    for r in MAIN_RESOLUTIONS {
        let (mut s, _) = store(None);
        s.set("main_resolution", r, secs(0)).unwrap();
        let d = s.settings().main_stream().dim;
        assert_eq!(d.w * 3, d.h * 4, "{r}");
        assert_eq!(format!("{}x{}", d.w, d.h), *r);
    }
}

#[test]
fn a_burst_of_changes_is_saved_once_within_the_window_of_the_first() {
    let (mut s, _) = store(None);
    s.set("mic_volume", "50", secs(0)).unwrap();
    s.tick(secs(5)).unwrap();
    s.set("main_bitrate", "2048", secs(9)).unwrap();
    s.tick(secs(9)).unwrap();
    assert!(
        s.disk().ops.iter().all(|o| *o == "read"),
        "nothing written yet"
    );
    // The first change is 10 s old: it is written now, with the second, never postponed.
    s.tick(secs(10)).unwrap();
    assert_eq!(
        s.disk().ops,
        ["read", "write_temp", "sync_temp", "rename_temp", "sync_dir"]
    );
    let file = s.disk().file.clone().unwrap();
    assert!(file.contains("mic_volume = 50") && file.contains("main_bitrate = 2048"));
    assert!(!s.dirty());
    // The next change opens a new window; writes stay at least 10 s apart.
    s.set("mic_volume", "60", secs(11)).unwrap();
    s.tick(secs(20)).unwrap();
    assert_eq!(s.disk().ops.len(), 5);
    s.tick(secs(21)).unwrap();
    assert_eq!(s.disk().ops.len(), 9);
}

#[test]
fn a_saved_file_loads_back_to_the_same_settings() {
    let (mut s, _) = store(None);
    s.set("aec", "ON", secs(0)).unwrap();
    s.set("ir", "night", secs(0)).unwrap();
    s.set("main_resolution", "1280x960", secs(0)).unwrap();
    s.flush().unwrap();
    let saved = s.settings().clone();
    let (again, problems) = store(s.disk().file.as_deref());
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(again.settings(), &saved);
}

#[test]
fn a_failed_write_stays_dirty_is_reported_and_retried_a_window_later() {
    let (mut s, _) = store(None);
    s.disk_mut().fail_write = true;
    s.set("mic_volume", "50", secs(0)).unwrap();
    assert!(s.tick(secs(10)).is_err());
    assert!(s.dirty());
    assert_eq!(summary(&s.problems()), "invalid: file");
    s.disk_mut().fail_write = false;
    s.tick(secs(15)).unwrap();
    assert!(s.dirty(), "retry waits one window");
    s.tick(secs(20)).unwrap();
    assert!(!s.dirty());
    assert!(s.problems().is_empty());
}

#[test]
fn flush_writes_only_when_something_is_unsaved() {
    let (mut s, _) = store(None);
    s.flush().unwrap();
    assert_eq!(s.disk().ops, ["read"]);
    s.set("led", "OFF", secs(0)).unwrap();
    s.flush().unwrap();
    assert!(s.disk().file.as_deref().unwrap().contains("led = false"));
}

#[test]
fn states_render_every_setting_for_mqtt() {
    let (s, _) = store(None);
    let states: BTreeMap<_, _> = s.settings().states().collect();
    assert_eq!(states.len(), specs().len());
    assert_eq!(states["aec"], "OFF");
    assert_eq!(states["main_resolution"], "1920x1440");
    assert_eq!(states["mic_volume"], "100");
}

#[test]
fn docs_settings_md_carries_the_table_rendered_from_the_schema() {
    let doc = include_str!("../../../docs/settings.md");
    let table = markdown_table();
    assert!(
        doc.contains(&table),
        "docs/settings.md is out of date; paste:\n{table}"
    );
}

#[test]
fn autofocus_starts_on_with_a_thirty_second_sixty_percent_watchdog_and_validates_its_ranges() {
    let (mut s, _) = store(None);
    assert_eq!(
        s.settings().autofocus(),
        crate::zoom::af::AfConfig {
            enabled: true,
            watch_hold: secs(30),
            watch_drop_pct: 60,
            move_keep_pct: 80,
            climb_drop_pct: 2,
        }
    );
    for (key, bad) in [
        ("af_watch_hold", "9"),
        ("af_watch_hold", "301"),
        ("af_watch_drop", "19"),
        ("af_watch_drop", "91"),
        ("af_move_drop", "49"),
        ("af_move_drop", "100"),
        ("af_climb_drop", "0"),
        ("af_climb_drop", "21"),
        ("tap_delay", "-1"),
        ("tap_delay", "3001"),
        ("autofocus", "yes"),
    ] {
        assert!(s.set(key, bad, secs(0)).is_err(), "{key}={bad}");
    }
    s.set("autofocus", "OFF", secs(0)).unwrap();
    s.set("af_watch_hold", "120", secs(0)).unwrap();
    s.set("af_watch_drop", "40", secs(0)).unwrap();
    s.set("af_move_drop", "90", secs(0)).unwrap();
    s.set("af_climb_drop", "5", secs(0)).unwrap();
    let a = s.settings().autofocus();
    assert_eq!(
        (
            a.enabled,
            a.watch_hold,
            a.watch_drop_pct,
            a.move_keep_pct,
            a.climb_drop_pct
        ),
        (false, secs(120), 40, 90, 5)
    );
    let (s, problems) = store(Some(
        "autofocus = false\naf_watch_hold = 45\naf_watch_drop = 300\naf_move_drop = 70\naf_climb_drop = 3\n",
    ));
    assert_eq!(problems.len(), 1, "{problems:?}");
    let a = s.settings().autofocus();
    assert_eq!(
        (
            a.enabled,
            a.watch_hold,
            a.watch_drop_pct,
            a.move_keep_pct,
            a.climb_drop_pct
        ),
        (false, secs(45), 60, 70, 3)
    );
}

#[test]
fn every_setting_but_the_status_led_sits_on_a_section_device_with_a_safe_unique_slug() {
    for s in specs() {
        match s.section() {
            None => assert_eq!(s.spec.key, "led", "{} has no section", s.spec.key),
            Some(sec) => assert!(!sec.name().is_empty()),
        }
    }
    let slugs: Vec<&str> = Section::ALL.iter().map(|s| s.slug()).collect();
    for (i, slug) in slugs.iter().enumerate() {
        assert!(
            slug.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
            "{slug}"
        );
        assert!(!slugs[..i].contains(slug), "{slug} twice");
    }
    assert_eq!(Section::ALL.len(), 6);
    // the section membership from the apply groups
    let on = |key: &str| spec(key).unwrap().section();
    assert_eq!(on("mic_volume"), Some(Section::Audio));
    assert_eq!(on("mirror"), Some(Section::Picture));
    assert_eq!(on("isp_brightness"), Some(Section::Picture));
    assert_eq!(on("main_bitrate"), Some(Section::MainStream));
    assert_eq!(on("ir"), Some(Section::NightVision));
    assert_eq!(on("ptz_ramp"), Some(Section::PanTilt));
    assert_eq!(on("zoom_speed"), Some(Section::Lens));
    assert_eq!(on("af_watch_drop"), Some(Section::Lens));
}

#[test]
fn the_tap_delay_is_a_pan_tilt_setting_in_milliseconds() {
    let (mut s, _) = store(None);
    assert_eq!(
        s.settings().tap_delay(),
        secs(0) + Duration::from_millis(TAP_DELAY_DEFAULT_MS as u64)
    );
    s.set("tap_delay", "250", secs(0)).unwrap();
    assert_eq!(s.settings().tap_delay(), Duration::from_millis(250));
    assert_eq!(spec("tap_delay").unwrap().section(), Some(Section::PanTilt));
}

const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/settings-2026-11-07/"
);

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}{name}")).unwrap()
}

#[test]
fn files_written_before_the_shared_store_load_and_are_rewritten_byte_for_byte() {
    for name in ["first-run.toml", "changed.toml"] {
        let text = fixture(name);
        let (mut s, problems) = store(Some(&text));
        assert!(problems.is_empty(), "{name}: {problems:?}");
        // A change and its undo leave the values as loaded but force a write.
        let led = if s.settings().led() { "OFF" } else { "ON" };
        let back = if led == "ON" { "OFF" } else { "ON" };
        s.set("led", led, secs(0)).unwrap();
        s.set("led", back, secs(0)).unwrap();
        s.flush().unwrap();
        assert_eq!(s.disk().file.as_deref(), Some(text.as_str()), "{name}");
    }
    let (s, _) = store(Some(&fixture("changed.toml")));
    let s = s.settings();
    assert_eq!(s.audio().mic_volume, 55);
    assert!(s.audio().aec.enabled);
    assert_eq!(s.ir_mode(), "night");
    assert_eq!(s.main_stream().dim.w, 2592);
    assert_eq!(s.motion().0.max_pps, 600);
    assert_eq!(s.get("isp_anti_flicker"), Some(1));
    assert_eq!(s.zoom_speed(), 40);
    assert!(
        !s.orientation().mirror(),
        "the file wins over the stock mirror"
    );
    let (s, problems) = store(Some(&fixture("hand-edited.toml")));
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(s.settings().audio().mic_volume, 55);
    assert!(
        s.settings().orientation().mirror(),
        "absent: the stock value"
    );
}

#[test]
fn values_are_i32_slots_shared_with_the_reader() {
    let (mut s, _) = store(None);
    let reader = s.reader();
    let feed = reader.subscribe();
    let ir = spec("ir").unwrap();
    s.set("ir", "day", secs(0)).unwrap();
    s.set("aec", "ON", secs(0)).unwrap();
    assert_eq!(
        reader.get(ir.spec.slot),
        Some(1),
        "a select is its option index"
    );
    assert_eq!(reader.value("aec"), Some(1), "a switch is 0/1");
    assert_eq!(reader.text("ir").as_deref(), Some("day"));
    assert_eq!(
        feed.try_recv(),
        Ok(shared::Event::Changed {
            slot: ir.spec.slot,
            value: 1
        })
    );
    // A rejected cross-field change reaches neither the snapshot nor the reader.
    assert!(s.set("ir_to_day", "2400", secs(0)).is_err());
    assert_eq!(reader.value("ir_to_day"), Some(1900));
    assert_eq!(s.settings().get("ir_to_day"), Some(1900));
}
