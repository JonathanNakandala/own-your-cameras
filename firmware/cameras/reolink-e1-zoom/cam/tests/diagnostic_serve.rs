//! Actual diagnostic command assembly with only camera hardware replaced.
use hdal::{Step, fake::Fake};
use std::sync::{Arc, Mutex, atomic::AtomicBool};

#[test]
fn diagnostic_reports_service_and_media_cleanup_errors_after_joining() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("cam-diag-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let config = root.join("cam.toml");
    std::fs::write(&config, "[mqtt]\nhost='127.0.0.1'\n").unwrap();
    std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
    let d = cam::cmd::serve::Diagnostic {
        port: 0,
        auth: None,
        seconds: 2,
        config_path: config.to_string_lossy().into_owned(),
        config_required: true,
        retunes: Vec::new(),
    };
    let trace = Arc::new(Mutex::new(Vec::new()));
    let mut hw = Fake::failing(Step::AudioCapStop, -77);
    hw.trace = Some(Arc::clone(&trace));
    let stop = Box::leak(Box::new(AtomicBool::new(false)));
    let service_trace = Arc::clone(&trace);
    let result = cam::cmd::serve::diagnostic_with(
        &d,
        hw,
        stop,
        Box::new(move |_| {
            service_trace.lock().unwrap().push("hardware_open_failed");
            Err("diagnostic hardware fault".into())
        }),
    );
    std::fs::remove_dir_all(root).unwrap();
    let error = result.unwrap_err().to_string();
    assert!(error.contains("diagnostic hardware fault"), "{error}");
    assert!(error.contains("-77"), "{error}");
    let trace = trace.lock().unwrap();
    let at = |name| trace.iter().position(|e| *e == name).unwrap();
    assert!(at("hardware_open_failed") < at("audio_cap_stop"));
    assert!(at("audio_cap_stop") < at("common_uninit"));
}
