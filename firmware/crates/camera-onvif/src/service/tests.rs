//! Every action's response shape, checked against the element names of the ONVIF WSDLs.
use super::*;
use crate::fake::FakeCamera;
use crate::soap;
use crate::xml::{self, Element};
use std::sync::atomic::Ordering;
use std::time::UNIX_EPOCH;

const NOW: u64 = 1_794_052_800; // 2026-11-07T12:00:00Z

fn ctx(camera: &FakeCamera) -> Context<'_> {
    Context {
        now: UNIX_EPOCH + Duration::from_secs(NOW),
        http_host: "CAMERA_HOST:80",
        host: "CAMERA_HOST",
        rtsp_port: 554,
        http_port: 80,
        camera,
        user: "USER",
        subscription: None,
    }
}

fn request(ns: &str, action: &str, body: &str) -> Request {
    let doc = format!(
        r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:tt="http://www.onvif.org/ver10/schema"><s:Body><{action} xmlns="{ns}">{body}</{action}></s:Body></s:Envelope>"#
    );
    soap::parse(doc.as_bytes()).unwrap()
}

/// The response's Body element, after a round trip through the XML reader.
fn call(camera: &FakeCamera, ns: &str, action: &str, body: &str) -> Element {
    let mut book = PullPointBook::new(camera.digital_inputs());
    call_with(camera, &mut book, ns, action, body)
}

fn call_with(
    camera: &FakeCamera,
    book: &mut PullPointBook,
    ns: &str,
    action: &str,
    body: &str,
) -> Element {
    let xml = match handle(&request(ns, action, body), &ctx(camera), book) {
        Reply::Ok(b) | Reply::Stopped(b) | Reply::Moving { body: b, .. } => soap::envelope("", &b),
        Reply::Fault(f) => panic!("{action}: {f:?}"),
        Reply::Pull { .. } => panic!("{action}: pull"),
    };
    let root = xml::parse(xml.as_bytes()).unwrap();
    let response = root.child("Body").unwrap().children[0].clone();
    assert_eq!(response.name, format!("{action}Response"));
    response
}

fn fault(camera: &FakeCamera, ns: &str, action: &str, body: &str) -> Fault {
    let mut book = PullPointBook::new(&[]);
    match handle(&request(ns, action, body), &ctx(camera), &mut book) {
        Reply::Fault(f) => f,
        other => panic!("{action}: expected a fault, got {other:?}"),
    }
}

fn path<'a>(e: &'a Element, names: &[&str]) -> &'a Element {
    names.iter().fold(e, |at, n| {
        at.child(n)
            .unwrap_or_else(|| panic!("no {n} in {}", at.name))
    })
}

#[test]
fn device_information_carries_the_runtime_serial() {
    let cam = FakeCamera::new(false, &[]);
    let r = call(&cam, TDS, "GetDeviceInformation", "");
    for (name, value) in [
        ("Manufacturer", "Maker"),
        ("Model", "Model X"),
        ("FirmwareVersion", "test"),
        ("SerialNumber", "020000000001"),
        ("HardwareId", "rev1"),
    ] {
        assert_eq!(r.text_of(name), Some(value), "{name}");
    }
}

#[test]
fn capabilities_list_device_events_media_and_ptz_only_when_present() {
    let cam = FakeCamera::new(true, &["door"]);
    let caps_r = call(&cam, TDS, "GetCapabilities", "<Category>All</Category>");
    let caps = path(&caps_r, &["Capabilities"]);
    assert_eq!(
        path(caps, &["Device", "XAddr"]).text,
        "http://CAMERA_HOST:80/onvif/device_service"
    );
    assert_eq!(path(caps, &["Device", "IO", "InputConnectors"]).text, "1");
    assert_eq!(path(caps, &["Events", "WSPullPointSupport"]).text, "true");
    assert_eq!(
        path(caps, &["Media", "StreamingCapabilities", "RTP_RTSP_TCP"]).text,
        "true"
    );
    assert_eq!(
        path(caps, &["PTZ", "XAddr"]).text,
        "http://CAMERA_HOST:80/onvif/ptz_service"
    );
    let media_only_r = call(&cam, TDS, "GetCapabilities", "<Category>Media</Category>");
    let media_only = path(&media_only_r, &["Capabilities"]);
    assert_eq!(media_only.children.len(), 1);
    let panel = FakeCamera::new(false, &[]);
    let caps = call(&panel, TDS, "GetCapabilities", "");
    assert!(path(&caps, &["Capabilities"]).child("PTZ").is_none());
    assert!(
        fault(
            &panel,
            TDS,
            "GetCapabilities",
            "<Category>Imaging</Category>"
        )
        .subcodes
        .contains(&"ter:NoSuchService")
    );
}

#[test]
fn services_name_each_namespace_with_capabilities_on_request() {
    let cam = FakeCamera::new(true, &[]);
    let r = call(
        &cam,
        TDS,
        "GetServices",
        "<IncludeCapability>true</IncludeCapability>",
    );
    let services: Vec<&Element> = r.children_named("Service").collect();
    assert_eq!(services.len(), 6);
    let imaging = services
        .iter()
        .find(|s| s.text_of("Namespace") == Some(TIMG))
        .unwrap();
    assert_eq!(
        imaging.text_of("XAddr"),
        Some("http://CAMERA_HOST:80/onvif/imaging_service")
    );
    assert_eq!(
        path(imaging, &["Capabilities", "Capabilities"]).attr("Presets"),
        Some("false")
    );
    let io = services
        .iter()
        .find(|s| s.text_of("Namespace") == Some(crate::soap::TMD))
        .unwrap();
    assert_eq!(
        path(io, &["Capabilities", "Capabilities"]).attr("AudioOutputs"),
        Some("1")
    );
    let media = services
        .iter()
        .find(|s| s.text_of("Namespace") == Some(TRT))
        .unwrap();
    assert_eq!(
        media.text_of("XAddr"),
        Some("http://CAMERA_HOST:80/onvif/media_service")
    );
    let caps = path(media, &["Capabilities", "Capabilities"]);
    assert_eq!(caps.attr("SnapshotUri"), Some("true"));
    assert!(path(media, &["Version"]).child("Major").is_some());
    let panel = FakeCamera::new(false, &[]);
    assert_eq!(
        call(&panel, TDS, "GetServices", "")
            .children_named("Service")
            .count(),
        4,
        "no PTZ service without PTZ"
    );
}

#[test]
fn service_capabilities_answer_in_each_namespace() {
    let cam = FakeCamera::new(true, &[]);
    let device = call(&cam, TDS, "GetServiceCapabilities", "");
    assert_eq!(
        path(&device, &["Capabilities", "Security"]).attr("UsernameToken"),
        Some("true")
    );
    let media = call(&cam, TRT, "GetServiceCapabilities", "");
    assert_eq!(
        path(&media, &["Capabilities", "ProfileCapabilities"]).attr("MaximumNumberOfProfiles"),
        Some("2")
    );
    let ptz = call(&cam, TPTZ, "GetServiceCapabilities", "");
    assert_eq!(
        path(&ptz, &["Capabilities"]).attr("MoveStatus"),
        Some("true")
    );
    let ev = call(&cam, TEV, "GetServiceCapabilities", "");
    assert_eq!(
        path(&ev, &["Capabilities"]).attr("MaxPullPoints"),
        Some("4")
    );
}

#[test]
fn date_and_time_report_ntp_and_the_true_utc_clock() {
    let cam = FakeCamera::new(false, &[]);
    let r_r = call(&cam, TDS, "GetSystemDateAndTime", "");
    let r = path(&r_r, &["SystemDateAndTime"]);
    assert_eq!(r.text_of("DateTimeType"), Some("NTP"));
    let utc = path(r, &["UTCDateTime"]);
    assert_eq!(
        utc.children[0].name, "Time",
        "tt:DateTime is Time, then Date"
    );
    assert_eq!(path(utc, &["Date", "Year"]).text, "2026");
    assert_eq!(path(utc, &["Date", "Day"]).text, "7");
    assert_eq!(path(utc, &["Time", "Hour"]).text, "12");
}

#[test]
fn scopes_hostname_network_dns_and_ntp_are_read_only_views() {
    let cam = FakeCamera::new(false, &[]);
    let scopes: Vec<String> = call(&cam, TDS, "GetScopes", "")
        .children_named("Scopes")
        .map(|s| s.text_of("ScopeItem").unwrap().to_string())
        .collect();
    assert!(scopes.contains(&"onvif://www.onvif.org/Profile/Streaming".to_string()));
    assert_eq!(
        path(
            &call(&cam, TDS, "GetHostname", ""),
            &["HostnameInformation", "Name"]
        )
        .text,
        "camera"
    );
    let nic_r = call(&cam, TDS, "GetNetworkInterfaces", "");
    let nic = path(&nic_r, &["NetworkInterfaces"]);
    assert_eq!(nic.attr("token"), Some("eth0"));
    assert_eq!(path(nic, &["Info", "HwAddress"]).text, "02:00:00:00:00:01");
    assert_eq!(
        path(nic, &["IPv4", "Config", "FromDHCP", "Address"]).text,
        "CAMERA_HOST"
    );
    assert_eq!(path(nic, &["IPv4", "Config", "DHCP"]).text, "true");
    let dns_r = call(&cam, TDS, "GetDNS", "");
    let dns = path(&dns_r, &["DNSInformation"]);
    assert_eq!(
        path(dns, &["DNSFromDHCP", "IPv4Address"]).text,
        "192.0.2.53"
    );
    let ntp_r = call(&cam, TDS, "GetNTP", "");
    let ntp = path(&ntp_r, &["NTPInformation", "NTPManual"]);
    assert_eq!(
        (ntp.text_of("Type"), ntp.text_of("DNSname")),
        (Some("DNS"), Some("pool.ntp.org"))
    );
    let protocols = call(&cam, TDS, "GetNetworkProtocols", "");
    assert_eq!(protocols.children_named("NetworkProtocols").count(), 2);
}

#[test]
fn writes_and_unknown_device_actions_are_not_supported() {
    let cam = FakeCamera::new(false, &[]);
    for action in [
        "SetSystemDateAndTime",
        "SystemReboot",
        "SetHostname",
        "CreateUsers",
        "SetNTP",
    ] {
        assert_eq!(
            fault(&cam, TDS, action, "").subcodes,
            ["ter:ActionNotSupported"],
            "{action}"
        );
    }
    assert_eq!(fault(&cam, "urn:other", "Anything", "").status(), 500);
}

#[test]
fn device_reboots_lists_its_user_and_serves_its_log() {
    let cam = FakeCamera::new(true, &[]);
    let rebooted = call(&cam, TDS, "SystemReboot", "");
    assert_eq!(rebooted.text_of("Message"), Some("Rebooting"));
    assert_eq!(
        cam.system.as_ref().unwrap().reboots.load(Ordering::SeqCst),
        1
    );
    let users = call(&cam, TDS, "GetUsers", "");
    assert_eq!(users.children_named("User").count(), 1);
    assert_eq!(path(&users, &["User", "Username"]).text, "USER");
    assert_eq!(path(&users, &["User", "UserLevel"]).text, "Administrator");
    // The log is escaped on the way out and back (the test's reader trims the final newline).
    let log = call(&cam, TDS, "GetSystemLog", "<LogType>System</LogType>");
    assert_eq!(
        path(&log, &["SystemLog", "String"]).text,
        "talk: session ended <ok> & done"
    );
    let info = call(&cam, TDS, "GetSystemSupportInformation", "");
    let text = &path(&info, &["SupportInformation", "String"]).text;
    assert!(text.contains("Model: ") && text.contains("Serial: ") && text.contains("Profile S"));
    // The log is advertised in the capability documents.
    let caps = call(&cam, TDS, "GetServiceCapabilities", "");
    assert_eq!(
        path(&caps, &["Capabilities", "System"]).attr("SystemLogging"),
        Some("true")
    );
    let all = call(&cam, TDS, "GetCapabilities", "<Category>Device</Category>");
    assert_eq!(
        path(&all, &["Capabilities", "Device", "System", "SystemLogging"]).text,
        "true"
    );
}

#[test]
fn device_refuses_what_the_camera_does_not_keep() {
    let cam = FakeCamera::new(true, &[]);
    assert!(
        fault(&cam, TDS, "GetSystemLog", "<LogType>Access</LogType>")
            .subcodes
            .contains(&"ter:SystemlogUnavailable")
    );
    for bad in ["", "<LogType>Debug</LogType>"] {
        assert!(
            fault(&cam, TDS, "GetSystemLog", bad)
                .subcodes
                .contains(&"ter:InvalidArgs"),
            "{bad:?}"
        );
    }
    // A camera that keeps no log and cannot reboot says so, and does not advertise either.
    let panel = FakeCamera::new(false, &[]);
    assert!(
        fault(&panel, TDS, "GetSystemLog", "<LogType>System</LogType>")
            .subcodes
            .contains(&"ter:SystemlogUnavailable")
    );
    assert_eq!(
        fault(&panel, TDS, "SystemReboot", "").subcodes,
        ["ter:ActionNotSupported"]
    );
    let caps = call(&panel, TDS, "GetServiceCapabilities", "");
    assert_eq!(
        path(&caps, &["Capabilities", "System"]).attr("SystemLogging"),
        Some("false")
    );
}

#[test]
fn media_accepts_a_sync_point_and_lists_no_metadata() {
    let cam = FakeCamera::new(true, &[]);
    call(
        &cam,
        TRT,
        "SetSynchronizationPoint",
        "<ProfileToken>sub</ProfileToken>",
    );
    assert!(
        fault(
            &cam,
            TRT,
            "SetSynchronizationPoint",
            "<ProfileToken>nope</ProfileToken>"
        )
        .subcodes
        .contains(&"ter:NoProfile")
    );
    let none = call(&cam, TRT, "GetMetadataConfigurations", "");
    assert!(none.children.is_empty());
}

#[test]
fn profiles_hold_every_configuration_in_schema_order() {
    let cam = FakeCamera::new(true, &[]);
    let r = call(&cam, TRT, "GetProfiles", "");
    let profiles: Vec<&Element> = r.children_named("Profiles").collect();
    assert_eq!(profiles.len(), 2);
    let main = profiles[0];
    assert_eq!(
        (main.attr("token"), main.attr("fixed")),
        (Some("main"), Some("true"))
    );
    let order: Vec<&str> = main.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        order,
        [
            "Name",
            "VideoSourceConfiguration",
            "AudioSourceConfiguration",
            "VideoEncoderConfiguration",
            "AudioEncoderConfiguration",
            "PTZConfiguration",
            "Extension"
        ]
    );
    let venc = path(main, &["VideoEncoderConfiguration"]);
    assert_eq!(venc.attr("token"), Some("main_video"));
    assert_eq!(path(venc, &["Resolution", "Width"]).text, "2560");
    assert_eq!(path(venc, &["RateControl", "FrameRateLimit"]).text, "15");
    assert_eq!(path(venc, &["RateControl", "BitrateLimit"]).text, "2048");
    assert_eq!(path(venc, &["H264", "GovLength"]).text, "30");
    assert_eq!(path(venc, &["H264", "H264Profile"]).text, "Main");
    assert_eq!(
        path(main, &["AudioEncoderConfiguration", "Encoding"]).text,
        "G711"
    );
    assert_eq!(
        path(main, &["AudioEncoderConfiguration", "SampleRate"]).text,
        "8"
    );
    assert_eq!(
        path(main, &["VideoSourceConfiguration", "Bounds"]).attr("width"),
        Some("2560")
    );
    assert_eq!(
        path(main, &["PTZConfiguration", "NodeToken"]).text,
        "ptz_node"
    );
    assert_eq!(
        path(
            main,
            &["Extension", "AudioOutputConfiguration", "OutputToken"]
        )
        .text,
        "audio_output"
    );
    assert!(
        path(main, &["Extension"])
            .child("AudioDecoderConfiguration")
            .is_some()
    );
    let sub = call(&cam, TRT, "GetProfile", "<ProfileToken>sub</ProfileToken>");
    assert_eq!(
        path(
            &sub,
            &[
                "Profile",
                "VideoEncoderConfiguration",
                "Resolution",
                "Height"
            ]
        )
        .text,
        "360"
    );
    assert!(
        fault(&cam, TRT, "GetProfile", "<ProfileToken>x</ProfileToken>")
            .subcodes
            .contains(&"ter:NoProfile")
    );
}

#[test]
fn stream_uris_name_the_pcmu_paths_and_snapshots_the_http_one() {
    let cam = FakeCamera::new(false, &[]);
    let body = "<StreamSetup><tt:Stream>RTP-Unicast</tt:Stream><tt:Transport><tt:Protocol>RTSP</tt:Protocol></tt:Transport></StreamSetup><ProfileToken>sub</ProfileToken>";
    let uri_r = call(&cam, TRT, "GetStreamUri", body);
    let uri = path(&uri_r, &["MediaUri", "Uri"]);
    assert_eq!(uri.text, "rtsp://CAMERA_HOST:554/sub_pcmu");
    let snap_r = call(
        &cam,
        TRT,
        "GetSnapshotUri",
        "<ProfileToken>main</ProfileToken>",
    );
    let snap = path(&snap_r, &["MediaUri", "Uri"]);
    assert_eq!(snap.text, "http://CAMERA_HOST:80/snapshot.jpg");
}

#[test]
fn sources_options_and_counts_report_the_current_settings_only() {
    let cam = FakeCamera::new(false, &[]);
    let src_r = call(&cam, TRT, "GetVideoSources", "");
    let src = path(&src_r, &["VideoSources"]);
    assert_eq!(
        (src.attr("token"), src.text_of("Framerate")),
        (Some("video_source"), Some("15"))
    );
    let opts_r = call(
        &cam,
        TRT,
        "GetVideoEncoderConfigurationOptions",
        "<ConfigurationToken>sub_video</ConfigurationToken>",
    );
    let opts = path(&opts_r, &["Options"]);
    let h264 = path(opts, &["H264"]);
    assert_eq!(h264.children_named("ResolutionsAvailable").count(), 1);
    assert_eq!(path(h264, &["FrameRateRange", "Max"]).text, "15");
    assert_eq!(
        path(opts, &["Extension", "H264", "BitrateRange", "Max"]).text,
        "256"
    );
    let all_r = call(&cam, TRT, "GetVideoEncoderConfigurationOptions", "");
    let all = path(&all_r, &["Options", "H264"]);
    assert_eq!(all.children_named("ResolutionsAvailable").count(), 2);
    let vso_r = call(&cam, TRT, "GetVideoSourceConfigurationOptions", "");
    let vso = path(&vso_r, &["Options", "BoundsRange", "WidthRange", "Max"]);
    assert_eq!(vso.text, "2560");
    let n = call(&cam, TRT, "GetGuaranteedNumberOfVideoEncoderInstances", "");
    assert_eq!(n.text_of("TotalNumber"), Some("2"));
    assert_eq!(
        call(&cam, TRT, "GetVideoEncoderConfigurations", "")
            .children_named("Configurations")
            .count(),
        2
    );
    assert_eq!(
        path(
            &call(&cam, TRT, "GetAudioSources", ""),
            &["AudioSources", "Channels"]
        )
        .text,
        "1"
    );
    assert_eq!(
        path(
            &call(&cam, TRT, "GetAudioEncoderConfigurationOptions", ""),
            &["Options", "Options", "Encoding"]
        )
        .text,
        "G711"
    );
    assert_eq!(
        path(&call(&cam, TRT, "GetAudioOutputs", ""), &["AudioOutputs"]).attr("token"),
        Some("audio_output")
    );
    assert!(
        path(
            &call(&cam, TRT, "GetAudioDecoderConfigurationOptions", ""),
            &["Options"]
        )
        .child("G711DecOptions")
        .is_some()
    );
    for (action, token) in [
        ("GetVideoSourceConfiguration", "video_source_config"),
        ("GetVideoEncoderConfiguration", "main_video"),
        ("GetAudioSourceConfiguration", "audio_source_config"),
        ("GetAudioEncoderConfiguration", "audio_encoder"),
        ("GetAudioOutputConfiguration", "audio_output_config"),
        ("GetAudioDecoderConfiguration", "audio_decoder"),
    ] {
        let r = call(
            &cam,
            TRT,
            action,
            &format!("<ConfigurationToken>{token}</ConfigurationToken>"),
        );
        assert_eq!(
            path(&r, &["Configuration"]).attr("token"),
            Some(token),
            "{action}"
        );
    }
}

#[test]
fn encoder_sets_succeed_only_when_nothing_changes() {
    let cam = FakeCamera::new(false, &[]);
    let same = r#"<Configuration token="main_video"><tt:Name>main_video</tt:Name><tt:Encoding>H264</tt:Encoding><tt:Resolution><tt:Width>2560</tt:Width><tt:Height>1440</tt:Height></tt:Resolution><tt:RateControl><tt:FrameRateLimit>15</tt:FrameRateLimit><tt:BitrateLimit>2048</tt:BitrateLimit></tt:RateControl><tt:H264><tt:GovLength>30</tt:GovLength><tt:H264Profile>Main</tt:H264Profile></tt:H264></Configuration><ForcePersistence>true</ForcePersistence>"#;
    call(&cam, TRT, "SetVideoEncoderConfiguration", same);
    let changed = same.replace(">15<", ">25<");
    let f = fault(&cam, TRT, "SetVideoEncoderConfiguration", &changed);
    assert_eq!(f.subcodes, ["ter:InvalidArgVal", "ter:ConfigModify"]);
    assert!(
        fault(
            &cam,
            TRT,
            "SetVideoEncoderConfiguration",
            &same.replace("main_video\"", "x\"")
        )
        .subcodes
        .contains(&"ter:NoConfig")
    );
    call(
        &cam,
        TRT,
        "SetAudioEncoderConfiguration",
        r#"<Configuration token="audio_encoder"><tt:Encoding>G711</tt:Encoding><tt:SampleRate>8</tt:SampleRate></Configuration>"#,
    );
    fault(
        &cam,
        TRT,
        "SetAudioEncoderConfiguration",
        r#"<Configuration token="audio_encoder"><tt:Encoding>AAC</tt:Encoding></Configuration>"#,
    );
    call(
        &cam,
        TRT,
        "SetVideoSourceConfiguration",
        r#"<Configuration token="video_source_config"><tt:Bounds x="0" y="0" width="2560" height="1440"/></Configuration>"#,
    );
    fault(
        &cam,
        TRT,
        "SetVideoSourceConfiguration",
        r#"<Configuration token="video_source_config"><tt:Bounds width="1280" height="720"/></Configuration>"#,
    );
    call(
        &cam,
        TRT,
        "AddAudioEncoderConfiguration",
        "<ProfileToken>main</ProfileToken><ConfigurationToken>audio_encoder</ConfigurationToken>",
    );
    fault(
        &cam,
        TRT,
        "AddVideoEncoderConfiguration",
        "<ProfileToken>main</ProfileToken><ConfigurationToken>sub_video</ConfigurationToken>",
    );
}

#[test]
fn profiles_are_fixed() {
    let cam = FakeCamera::new(false, &[]);
    assert_eq!(
        fault(&cam, TRT, "CreateProfile", "<Name>SynoTestProfile</Name>").subcodes,
        ["ter:Action", "ter:MaxNVTProfiles"]
    );
    assert!(
        fault(
            &cam,
            TRT,
            "DeleteProfile",
            "<ProfileToken>main</ProfileToken>"
        )
        .subcodes
        .contains(&"ter:DeletionOfFixedProfile")
    );
    let req = request(TRT, "CreateProfile", "");
    assert!(
        public(&req),
        "Surveillance Station sends it without credentials"
    );
    assert!(!public(&request(TRT, "GetProfiles", "")));
    assert!(public(&request(TDS, "GetSystemDateAndTime", "")));
}

#[test]
fn a_camera_without_speaker_or_ptz_offers_neither() {
    struct Panel(FakeCamera);
    impl OnvifCamera for Panel {
        fn identity(&self) -> &crate::DeviceIdentity {
            self.0.identity()
        }
        fn profiles(&self) -> Vec<MediaProfile> {
            self.0.profiles()
        }
        fn audio_output(&self) -> bool {
            false
        }
        fn ptz(&self) -> Option<&dyn PtzPort> {
            None
        }
        fn digital_inputs(&self) -> &[&'static str] {
            &[]
        }
        fn snapshot(&self, d: std::time::Instant) -> Result<Vec<u8>, crate::SnapshotError> {
            self.0.snapshot(d)
        }
    }
    let panel = Panel(FakeCamera::new(false, &[]));
    let c = Context {
        camera: &panel,
        ..ctx(&panel.0)
    };
    let mut book = PullPointBook::new(&[]);
    let Reply::Ok(body) = handle(&request(TRT, "GetProfiles", ""), &c, &mut book) else {
        panic!()
    };
    assert!(!body.contains("PTZConfiguration") && !body.contains("AudioOutputConfiguration"));
    assert!(matches!(
        handle(&request(TRT, "GetAudioOutputs", ""), &c, &mut book),
        Reply::Fault(_)
    ));
    assert!(matches!(
        handle(&request(TPTZ, "GetNodes", ""), &c, &mut book),
        Reply::Fault(_)
    ));
}

#[test]
fn ptz_moves_stop_and_presets_go_through_the_port() {
    let cam = FakeCamera::new(true, &[]);
    let node_r = call(&cam, TPTZ, "GetNodes", "");
    let node = path(&node_r, &["PTZNode"]);
    assert_eq!(node.attr("token"), Some("ptz_node"));
    assert_eq!(node.text_of("MaximumNumberOfPresets"), Some("3"));
    assert!(
        path(node, &["SupportedPTZSpaces"])
            .child("ContinuousZoomVelocitySpace")
            .is_some()
    );
    assert_eq!(
        path(
            &call(&cam, TPTZ, "GetConfigurations", ""),
            &["PTZConfiguration"]
        )
        .attr("token"),
        Some("ptz_config")
    );
    let opts = call(
        &cam,
        TPTZ,
        "GetConfigurationOptions",
        "<ConfigurationToken>ptz_config</ConfigurationToken>",
    );
    assert_eq!(
        path(&opts, &["PTZConfigurationOptions", "PTZTimeout", "Max"]).text,
        "PT60S"
    );
    let mv = request(
        TPTZ,
        "ContinuousMove",
        r#"<ProfileToken>main</ProfileToken><Velocity><tt:PanTilt x="0.5" y="-2" space="x"/><tt:Zoom x="0.25"/></Velocity><Timeout>PT2S</Timeout>"#,
    );
    let mut book = PullPointBook::new(&[]);
    match handle(&mv, &ctx(&cam), &mut book) {
        Reply::Moving { stop_after, .. } => assert_eq!(stop_after, Duration::from_secs(2)),
        other => panic!("{other:?}"),
    }
    let status = call(&cam, TPTZ, "GetStatus", "<ProfileToken>main</ProfileToken>");
    assert_eq!(
        path(&status, &["PTZStatus", "MoveStatus", "PanTilt"]).text,
        "MOVING"
    );
    let untimed = request(
        TPTZ,
        "ContinuousMove",
        r#"<ProfileToken>main</ProfileToken><Velocity><tt:PanTilt x="1" y="0"/></Velocity>"#,
    );
    assert!(
        matches!(handle(&untimed, &ctx(&cam), &mut book), Reply::Moving { stop_after, .. } if stop_after == DEFAULT_MOVE)
    );
    assert!(matches!(
        handle(
            &request(TPTZ, "Stop", "<ProfileToken>main</ProfileToken>"),
            &ctx(&cam),
            &mut book
        ),
        Reply::Stopped(_)
    ));
    call(
        &cam,
        TPTZ,
        "GotoHomePosition",
        "<ProfileToken>main</ProfileToken>",
    );
    let token = call(
        &cam,
        TPTZ,
        "SetPreset",
        "<ProfileToken>main</ProfileToken><PresetName>Door</PresetName>",
    );
    assert_eq!(token.text_of("PresetToken"), Some("2"));
    let presets = call(
        &cam,
        TPTZ,
        "GetPresets",
        "<ProfileToken>main</ProfileToken>",
    );
    assert_eq!(presets.children_named("Preset").count(), 2);
    assert_eq!(path(&presets, &["Preset", "Name"]).text, "Default");
    call(
        &cam,
        TPTZ,
        "GotoPreset",
        "<ProfileToken>main</ProfileToken><PresetToken>2</PresetToken>",
    );
    call(
        &cam,
        TPTZ,
        "RemovePreset",
        "<ProfileToken>main</ProfileToken><PresetToken>2</PresetToken>",
    );
    assert!(
        fault(
            &cam,
            TPTZ,
            "GotoPreset",
            "<ProfileToken>main</ProfileToken><PresetToken>9</PresetToken>"
        )
        .subcodes
        .contains(&"ter:NoToken")
    );
    assert!(
        fault(&cam, TPTZ, "GetStatus", "<ProfileToken>nope</ProfileToken>")
            .subcodes
            .contains(&"ter:NoProfile")
    );
    assert_eq!(
        cam.ptz.as_ref().unwrap().calls(),
        ["move 0.5 -1 0.25", "move 1 0 0", "stop", "home", "goto 2"]
    );
}

/// What Frigate's autotracking reads: an FOV translation space in the options, a default one in the
/// configuration, and `MoveStatus` in the service capabilities.
#[test]
fn ptz_advertises_what_frigate_needs() {
    let cam = FakeCamera::new(true, &[]);
    let fov = "http://www.onvif.org/ver10/tptz/PanTiltSpaces/TranslationSpaceFov";
    let opts = call(
        &cam,
        TPTZ,
        "GetConfigurationOptions",
        "<ConfigurationToken>ptz_config</ConfigurationToken>",
    );
    let space = path(
        &opts,
        &[
            "PTZConfigurationOptions",
            "Spaces",
            "RelativePanTiltTranslationSpace",
        ],
    );
    assert_eq!(space.text_of("URI"), Some(fov));
    assert_eq!(path(space, &["XRange", "Min"]).text, "-1");
    assert_eq!(path(space, &["YRange", "Max"]).text, "1");
    let config = call(&cam, TPTZ, "GetConfigurations", "");
    let config = path(&config, &["PTZConfiguration"]);
    assert_eq!(
        config.text_of("DefaultRelativePanTiltTranslationSpace"),
        Some(fov)
    );
    assert!(config.child("DefaultAbsoluteZoomPositionSpace").is_some());
    assert!(config.child("DefaultPTZSpeed").is_some());
    let caps = call(&cam, TPTZ, "GetServiceCapabilities", "");
    let caps = path(&caps, &["Capabilities"]);
    assert_eq!(caps.attr("MoveStatus"), Some("true"));
    assert_eq!(caps.attr("StatusPosition"), Some("true"));
    // The schema's order: Position, then MoveStatus.
    let status = call(&cam, TPTZ, "GetStatus", "<ProfileToken>main</ProfileToken>");
    let status = path(&status, &["PTZStatus"]);
    let names: Vec<_> = status.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Position", "MoveStatus", "UtcTime"]);
    assert_eq!(
        path(status, &["Position", "PanTilt"]).attr("x"),
        Some("0.2500")
    );
}

#[test]
fn home_can_be_set_and_auxiliary_commands_are_listed_and_checked() {
    let cam = FakeCamera::new(true, &[]);
    let node = call(&cam, TPTZ, "GetNodes", "");
    let node = path(&node, &["PTZNode"]);
    assert_eq!(node.attr("FixedHomePosition"), Some("false"));
    let aux: Vec<_> = node
        .children_named("AuxiliaryCommands")
        .map(|c| c.text.as_str())
        .collect();
    assert_eq!(aux, ["tt:IRLamp|On", "tt:IRLamp|Off"]);
    call(
        &cam,
        TPTZ,
        "SetHomePosition",
        "<ProfileToken>main</ProfileToken>",
    );
    let sent = call(
        &cam,
        TPTZ,
        "SendAuxiliaryCommand",
        "<ProfileToken>main</ProfileToken><AuxiliaryData>tt:IRLamp|On</AuxiliaryData>",
    );
    assert_eq!(sent.text_of("AuxiliaryResponse"), Some("OK"));
    for bad in ["", "tt:Wiper|On", "tt:IRLamp|Auto"] {
        assert!(
            fault(
                &cam,
                TPTZ,
                "SendAuxiliaryCommand",
                &format!("<ProfileToken>main</ProfileToken><AuxiliaryData>{bad}</AuxiliaryData>")
            )
            .subcodes
            .contains(&"ter:InvalidArgs"),
            "{bad:?}"
        );
    }
    assert_eq!(
        cam.ptz.as_ref().unwrap().calls(),
        ["sethome", "aux tt:IRLamp|On"]
    );
    let caps = call(&cam, TPTZ, "GetServiceCapabilities", "");
    assert_eq!(
        path(&caps, &["Capabilities"]).attr("GetCompatibleConfigurations"),
        Some("true")
    );
    let compatible = call(
        &cam,
        TPTZ,
        "GetCompatibleConfigurations",
        "<ProfileToken>main</ProfileToken>",
    );
    assert_eq!(
        path(&compatible, &["PTZConfiguration"]).attr("token"),
        Some("ptz_config")
    );
    assert!(
        fault(
            &cam,
            TPTZ,
            "GetCompatibleConfigurations",
            "<ProfileToken>x</ProfileToken>"
        )
        .subcodes
        .contains(&"ter:NoProfile")
    );
}

#[test]
fn stop_stops_everything_unless_told_to_stop_neither() {
    let cam = FakeCamera::new(true, &[]);
    let stop = |flags: &str| {
        call(
            &cam,
            TPTZ,
            "Stop",
            &format!("<ProfileToken>main</ProfileToken>{flags}"),
        );
        cam.ptz.as_ref().unwrap().calls().len()
    };
    assert_eq!(stop(""), 1, "no flag: all");
    assert_eq!(stop("<PanTilt>true</PanTilt><Zoom>true</Zoom>"), 2);
    assert_eq!(stop("<Zoom>true</Zoom>"), 3, "one axis named: both stop");
    assert_eq!(
        stop("<PanTilt>false</PanTilt><Zoom>false</Zoom>"),
        3,
        "neither: nothing"
    );
}

#[test]
fn a_camera_with_a_fixed_home_and_no_aux_says_so() {
    struct Plain;
    impl PtzPort for Plain {
        fn zoom(&self) -> bool {
            false
        }
        fn max_presets(&self) -> usize {
            0
        }
        fn continuous_move(&self, _: crate::Velocity) -> Result<(), PtzError> {
            Ok(())
        }
        fn stop(&self) -> Result<(), PtzError> {
            Ok(())
        }
        fn home(&self) -> Result<(), PtzError> {
            Ok(())
        }
        fn presets(&self) -> Vec<crate::Preset> {
            Vec::new()
        }
        fn goto_preset(&self, _: &str) -> Result<(), PtzError> {
            Ok(())
        }
        fn set_preset(&self, _: Option<&str>, _: Option<&str>) -> Result<String, PtzError> {
            Err(PtzError::InvalidName)
        }
        fn remove_preset(&self, _: &str) -> Result<(), PtzError> {
            Ok(())
        }
        fn moving(&self) -> bool {
            false
        }
    }
    let cam = FakeCamera::new(true, &[]);
    let through = |action: &str, body: &str| match ptz_service(
        &request(TPTZ, action, body),
        &ctx(&cam),
        &Plain,
    ) {
        Reply::Ok(b) | Reply::Stopped(b) | Reply::Moving { body: b, .. } => b,
        Reply::Fault(f) => f.render(),
        Reply::Pull { .. } => unreachable!(),
    };
    let node = through("GetNodes", "");
    assert!(node.contains("FixedHomePosition=\"true\"") && !node.contains("AuxiliaryCommands"));
    assert!(through("SetHomePosition", "").contains("ActionNotSupported"));
    assert!(
        through(
            "SendAuxiliaryCommand",
            "<AuxiliaryData>tt:IRLamp|On</AuxiliaryData>"
        )
        .contains("InvalidArgs")
    );
}

#[test]
fn relative_and_absolute_moves_reach_the_port() {
    let cam = FakeCamera::new(true, &[]);
    let fov = "http://www.onvif.org/ver10/tptz/PanTiltSpaces/TranslationSpaceFov";
    call(
        &cam,
        TPTZ,
        "RelativeMove",
        &format!(
            r#"<ProfileToken>main</ProfileToken><Translation><tt:PanTilt x="0.25" y="-3" space="{fov}"/><tt:Zoom x="0.5"/></Translation><Speed><tt:PanTilt x="1" y="1"/></Speed>"#
        ),
    );
    // No space named: the default (FOV) applies; a pan/tilt-only move leaves zoom alone.
    call(
        &cam,
        TPTZ,
        "RelativeMove",
        r#"<ProfileToken>main</ProfileToken><Translation><tt:PanTilt x="-0.1" y="0"/></Translation>"#,
    );
    call(
        &cam,
        TPTZ,
        "AbsoluteMove",
        r#"<ProfileToken>main</ProfileToken><Position><tt:Zoom x="2"/></Position>"#,
    );
    assert_eq!(
        cam.ptz.as_ref().unwrap().calls(),
        [
            "relative Some((0.25, -1.0)) Some(0.5)",
            "relative Some((-0.1, 0.0)) None",
            "absolute None Some(1.0)",
        ]
    );
    for (action, body, subcode) in [
        (
            "RelativeMove",
            r#"<ProfileToken>main</ProfileToken><Translation><tt:PanTilt x="0.1" y="0.1" space="http://www.onvif.org/ver10/tptz/PanTiltSpaces/TranslationGenericSpace"/></Translation>"#,
            "ter:SpaceNotSupported",
        ),
        (
            "RelativeMove",
            r#"<ProfileToken>main</ProfileToken><Translation><tt:PanTilt x="nan" y="0"/></Translation>"#,
            "ter:InvalidPosition",
        ),
        (
            "RelativeMove",
            r#"<ProfileToken>main</ProfileToken><Translation/>"#,
            "ter:InvalidPosition",
        ),
        (
            "AbsoluteMove",
            r#"<ProfileToken>main</ProfileToken><Position><tt:PanTilt x="0.1"/></Position>"#,
            "ter:InvalidPosition",
        ),
    ] {
        assert!(
            fault(&cam, TPTZ, action, body).subcodes.contains(&subcode),
            "{action} {body}"
        );
    }
    assert_eq!(
        cam.ptz.as_ref().unwrap().calls().len(),
        3,
        "faults move nothing"
    );
}

#[test]
fn a_camera_without_relative_moves_refuses_them() {
    // The default trait methods: continuous only, nothing advertised.
    struct Plain;
    impl PtzPort for Plain {
        fn zoom(&self) -> bool {
            false
        }
        fn max_presets(&self) -> usize {
            0
        }
        fn continuous_move(&self, _: crate::Velocity) -> Result<(), PtzError> {
            Ok(())
        }
        fn stop(&self) -> Result<(), PtzError> {
            Ok(())
        }
        fn home(&self) -> Result<(), PtzError> {
            Ok(())
        }
        fn presets(&self) -> Vec<crate::Preset> {
            Vec::new()
        }
        fn goto_preset(&self, _: &str) -> Result<(), PtzError> {
            Ok(())
        }
        fn set_preset(&self, _: Option<&str>, _: Option<&str>) -> Result<String, PtzError> {
            Err(PtzError::InvalidName)
        }
        fn remove_preset(&self, _: &str) -> Result<(), PtzError> {
            Ok(())
        }
        fn moving(&self) -> bool {
            false
        }
    }
    let cam = FakeCamera::new(true, &[]);
    /// The rendered reply (or fault) of `action` on `ptz`, as the wire would carry it.
    fn through(ptz: &dyn PtzPort, cam: &FakeCamera, action: &str) -> String {
        let body = "<ProfileToken>main</ProfileToken><Translation><tt:Zoom x=\"1\"/></Translation>";
        match ptz_service(&request(TPTZ, action, body), &ctx(cam), ptz) {
            Reply::Ok(b) | Reply::Stopped(b) | Reply::Moving { body: b, .. } => b,
            Reply::Fault(f) => f.render(),
            Reply::Pull { .. } => unreachable!(),
        }
    }
    let r = through(&Plain, &cam, "GetConfigurationOptions");
    assert!(!r.contains("RelativePanTiltTranslationSpace") && !r.contains("AbsolutePanTilt"));
    assert!(r.contains("ContinuousPanTiltVelocitySpace"));
    let status = through(&Plain, &cam, "GetStatus");
    assert!(!status.contains("<tt:Position>"));
    let refused = through(&Plain, &cam, "RelativeMove");
    assert!(refused.contains("ActionNotSupported"), "{refused}");
}

const SOURCE: &str = "<VideoSourceToken>video_source</VideoSourceToken>";

#[test]
fn imaging_reports_settings_and_options_in_the_schemas_order() {
    let cam = FakeCamera::new(true, &[]);
    let got = call(&cam, TIMG, "GetImagingSettings", SOURCE);
    let settings = path(&got, &["ImagingSettings"]);
    let names: Vec<_> = settings.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Brightness",
            "ColorSaturation",
            "Contrast",
            "Focus",
            "IrCutFilter",
            "Sharpness"
        ]
    );
    assert_eq!(settings.text_of("Brightness"), Some("100"));
    assert_eq!(settings.text_of("IrCutFilter"), Some("AUTO"));
    assert_eq!(path(settings, &["Focus", "AutoFocusMode"]).text, "AUTO");
    let got = call(&cam, TIMG, "GetOptions", SOURCE);
    let options = path(&got, &["ImagingOptions"]);
    assert_eq!(path(options, &["Brightness", "Max"]).text, "200");
    assert_eq!(options.children_named("IrCutFilterModes").count(), 3);
    assert_eq!(
        path(options, &["Focus"])
            .children_named("AutoFocusModes")
            .count(),
        2
    );
    // The service is in the capability documents too.
    let caps = call(&cam, TIMG, "GetServiceCapabilities", "");
    assert_eq!(
        path(&caps, &["Capabilities"]).attr("ImageStabilization"),
        Some("false")
    );
    let all = call(&cam, TDS, "GetCapabilities", "");
    assert_eq!(
        path(&all, &["Capabilities", "Imaging", "XAddr"]).text,
        "http://CAMERA_HOST:80/onvif/imaging_service"
    );
}

#[test]
fn imaging_sets_what_is_sent_and_ignores_sections_the_camera_lacks() {
    let cam = FakeCamera::new(true, &[]);
    call(
        &cam,
        TIMG,
        "SetImagingSettings",
        &format!(
            r#"{SOURCE}<ImagingSettings><tt:Brightness>150.5</tt:Brightness><tt:IrCutFilter>OFF</tt:IrCutFilter><tt:Focus><tt:AutoFocusMode>MANUAL</tt:AutoFocusMode></tt:Focus><tt:WhiteBalance><tt:Mode>AUTO</tt:Mode></tt:WhiteBalance></ImagingSettings><ForcePersistence>true</ForcePersistence>"#
        ),
    );
    let got = call(&cam, TIMG, "GetImagingSettings", SOURCE);
    let settings = path(&got, &["ImagingSettings"]);
    assert_eq!(settings.text_of("Brightness"), Some("150.5"));
    assert_eq!(settings.text_of("IrCutFilter"), Some("OFF"));
    assert_eq!(path(settings, &["Focus", "AutoFocusMode"]).text, "MANUAL");
    assert_eq!(settings.text_of("Contrast"), Some("100"), "unsent stays");
    assert_eq!(cam.imaging.as_ref().unwrap().calls().len(), 1);
}

#[test]
fn imaging_refuses_what_it_cannot_do_before_changing_anything() {
    let cam = FakeCamera::new(true, &[]);
    let set = |body: &str| {
        fault(
            &cam,
            TIMG,
            "SetImagingSettings",
            &format!("{SOURCE}<ImagingSettings>{body}</ImagingSettings>"),
        )
    };
    for bad in [
        "<tt:Brightness>nan</tt:Brightness>",
        "<tt:Brightness>soon</tt:Brightness>",
        "<tt:Brightness>201</tt:Brightness>",
        "<tt:IrCutFilter>MAYBE</tt:IrCutFilter>",
        "<tt:Focus><tt:AutoFocusMode>SOMETIMES</tt:AutoFocusMode></tt:Focus>",
    ] {
        assert!(set(bad).subcodes.contains(&"ter:SettingsInvalid"), "{bad}");
    }
    let none = fault(&cam, TIMG, "SetImagingSettings", SOURCE);
    assert!(none.subcodes.contains(&"ter:SettingsInvalid"));
    // A valid field beside an invalid one: neither is applied.
    assert!(
        set("<tt:Contrast>10</tt:Contrast><tt:Brightness>999</tt:Brightness>")
            .subcodes
            .contains(&"ter:SettingsInvalid")
    );
    assert!(cam.imaging.as_ref().unwrap().calls().is_empty());
    assert!(
        fault(
            &cam,
            TIMG,
            "GetImagingSettings",
            "<VideoSourceToken>nope</VideoSourceToken>"
        )
        .subcodes
        .contains(&"ter:NoSource")
    );
    // Focus moves are not offered.
    for action in ["Move", "GetMoveOptions", "GetStatus", "Stop"] {
        assert_eq!(
            fault(&cam, TIMG, action, SOURCE).subcodes,
            ["ter:ActionNotSupported"],
            "{action}"
        );
    }
    // A camera without imaging offers no service.
    let panel = FakeCamera::new(false, &[]);
    assert!(matches!(
        handle(
            &request(TIMG, "GetImagingSettings", SOURCE),
            &ctx(&panel),
            &mut PullPointBook::new(&[])
        ),
        Reply::Fault(_)
    ));
}

#[test]
fn pull_point_lifecycle_over_the_service() {
    let cam = FakeCamera::new(false, &["call_button", "door", "tamper"]);
    let mut book = PullPointBook::new(cam.digital_inputs());
    let props = call_with(&cam, &mut book, TEV, "GetEventProperties", "");
    assert!(props.descendant("DigitalInput").is_some());
    let created = call_with(
        &cam,
        &mut book,
        TEV,
        "CreatePullPointSubscription",
        "<InitialTerminationTime>PT10M</InitialTerminationTime>",
    );
    let address = path(&created, &["SubscriptionReference", "Address"])
        .text
        .clone();
    assert_eq!(address, "http://CAMERA_HOST:80/onvif/subscription/1");
    assert_eq!(
        created.text_of("TerminationTime"),
        Some("2026-11-07T12:10:00Z")
    );
    let mut sub = ctx(&cam);
    sub.subscription = Some(1);
    let pull = request(
        TEV,
        "PullMessages",
        "<Timeout>PT60S</Timeout><MessageLimit>10</MessageLimit>",
    );
    assert_eq!(
        handle(&pull, &sub, &mut book),
        Reply::Pull {
            id: 1,
            wait: MAX_PULL_WAIT,
            limit: 10
        }
    );
    let (messages, expires) = book.pull(1, sub.now, 10).unwrap();
    let xml = soap::envelope("", &pull_response(sub.now, expires, &messages));
    let root = xml::parse(xml.as_bytes()).unwrap();
    let r = path(&root, &["Body", "PullMessagesResponse"]);
    let notes: Vec<&Element> = r.children_named("NotificationMessage").collect();
    assert_eq!(notes.len(), 3);
    assert_eq!(
        notes[0].text_of("Topic"),
        Some("tns1:Device/Trigger/DigitalInput")
    );
    let message = path(notes[1], &["Message", "Message"]);
    assert_eq!(message.attr("PropertyOperation"), Some("Initialized"));
    assert_eq!(
        path(message, &["Source", "SimpleItem"]).attr("Value"),
        Some("door")
    );
    assert_eq!(
        path(message, &["Data", "SimpleItem"]).attr("Value"),
        Some("false")
    );
    let mut renew = request(WSNT, "Renew", "<TerminationTime>PT1M</TerminationTime>");
    renew.to = Some(address.clone());
    let Reply::Ok(_) = handle(&renew, &ctx(&cam), &mut book) else {
        panic!("renew by To")
    };
    match handle(
        &request(TEV, "SetSynchronizationPoint", ""),
        &sub,
        &mut book,
    ) {
        Reply::Ok(b) => assert!(b.contains("SetSynchronizationPointResponse")),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        handle(&request(WSNT, "Unsubscribe", ""), &sub, &mut book),
        Reply::Ok(_)
    ));
    assert!(matches!(handle(&pull, &sub, &mut book), Reply::Fault(_)));
}

#[test]
fn a_camera_without_inputs_has_an_empty_topic_set() {
    let cam = FakeCamera::new(true, &[]);
    let props = call(&cam, TEV, "GetEventProperties", "");
    assert!(path(&props, &["TopicSet"]).children.is_empty());
}

#[test]
fn device_io_counts_the_audio_output_recorders_need_for_talkback() {
    let cam = FakeCamera::new(true, &[]);
    let caps = call(&cam, TDS, "GetCapabilities", "<Category>All</Category>");
    let io = path(&caps, &["Capabilities", "Extension", "DeviceIO"]);
    assert_eq!(io.text_of("AudioOutputs"), Some("1"));
    assert_eq!(
        io.text_of("XAddr"),
        Some("http://CAMERA_HOST:80/onvif/deviceio_service")
    );
    let outputs = call(&cam, crate::soap::TMD, "GetAudioOutputs", "");
    assert_eq!(outputs.text_of("Token"), Some("audio_output"));
    let media_only = call(&cam, TDS, "GetCapabilities", "<Category>Media</Category>");
    assert!(
        media_only
            .child("Capabilities")
            .and_then(|c| c.child("Extension"))
            .is_none()
    );
}
