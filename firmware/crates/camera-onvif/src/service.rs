//! One dispatch table for the Device, Media (Profile S), PTZ and Events services. Pure: the time,
//! the address the client reached and the subscription book are passed in; the adapter does the I/O,
//! waiting for events and stopping a timed move.
//!
//! Media and Device settings are read-only: a `Set*` whose values equal the current ones succeeds as a
//! no-op, any other value is `ter:InvalidArgVal`. PTZ moves and presets act through the camera's motion
//! owner. Unknown actions are `ter:ActionNotSupported`.

use crate::camera::{
    Absolute, MediaProfile, OnvifCamera, PtzError, PtzFeatures, PtzPort, Relative, RelayError,
    Velocity,
};
use crate::events::{self, EventError, Message, PullPointBook, Termination};
use crate::soap::{Fault, Request, TDS, TEV, TIMG, TMD, TPTZ, TR2, TRT, WSNT};
use crate::time;
use crate::xml::{Element, escape};
use std::time::{Duration, SystemTime};

const VIDEO_SOURCE: &str = "video_source";
const VIDEO_SOURCE_CONFIG: &str = "video_source_config";
const AUDIO_SOURCE: &str = "audio_source";
const AUDIO_SOURCE_CONFIG: &str = "audio_source_config";
const AUDIO_ENCODER: &str = "audio_encoder";
const AUDIO_OUTPUT: &str = "audio_output";
const AUDIO_OUTPUT_CONFIG: &str = "audio_output_config";
const AUDIO_DECODER: &str = "audio_decoder";
const PTZ_NODE: &str = "ptz_node";
const PTZ_CONFIG: &str = "ptz_config";
const PAN_TILT_VELOCITY: &str =
    "http://www.onvif.org/ver10/tptz/PanTiltSpaces/VelocityGenericSpace";
const ZOOM_VELOCITY: &str = "http://www.onvif.org/ver10/tptz/ZoomSpaces/VelocityGenericSpace";
const PAN_TILT_POSITION: &str =
    "http://www.onvif.org/ver10/tptz/PanTiltSpaces/PositionGenericSpace";
const ZOOM_POSITION: &str = "http://www.onvif.org/ver10/tptz/ZoomSpaces/PositionGenericSpace";
/// Pan/tilt as a fraction of the picture: what Frigate's autotracking requires of a camera.
const PAN_TILT_FOV: &str = "http://www.onvif.org/ver10/tptz/PanTiltSpaces/TranslationSpaceFov";
const ZOOM_TRANSLATION: &str = "http://www.onvif.org/ver10/tptz/ZoomSpaces/TranslationGenericSpace";
const PAN_TILT_SPEED: &str = "http://www.onvif.org/ver10/tptz/PanTiltSpaces/GenericSpeedSpace";
const ZOOM_SPEED: &str = "http://www.onvif.org/ver10/tptz/ZoomSpaces/ZoomGenericSpeedSpace";
/// A ContinuousMove without `Timeout` stops after this (`DefaultPTZTimeout`).
pub const DEFAULT_MOVE: Duration = Duration::from_secs(10);
pub const MAX_MOVE: Duration = Duration::from_secs(60);
/// The longest a PullMessages waits before it answers with no messages.
pub const MAX_PULL_WAIT: Duration = Duration::from_secs(20);
const MAX_PULL_MESSAGES: usize = 100;

pub struct Context<'a> {
    pub now: SystemTime,
    /// `host:port` the client reached the ONVIF service at; every URL handed out uses it.
    pub http_host: &'a str,
    /// The same host for RTSP URLs: an IP literal, bracketed for IPv6.
    pub host: &'a str,
    pub rtsp_port: u16,
    pub http_port: u16,
    pub camera: &'a dyn OnvifCamera,
    /// The one user name the camera knows (its RTSP user), for `GetUsers`.
    pub user: &'a str,
    /// The subscription a request path names (`/onvif/subscription/N`).
    pub subscription: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// A successful response body (inside `env:Body`).
    Ok(String),
    Fault(Fault),
    /// The adapter waits up to `wait` for messages, then answers with [`pull_response`].
    Pull {
        id: u32,
        wait: Duration,
        limit: usize,
    },
    /// The camera moves: the adapter stops it after `stop_after` unless a later move or Stop wins.
    Moving {
        body: String,
        stop_after: Duration,
    },
    /// The camera was stopped (or sent to a preset): cancel any pending timed stop.
    Stopped(String),
}

/// Actions any client may call before it authenticates. `CreateProfile` is answered with a fault before
/// authentication: Synology Surveillance Station probes it without credentials while adding a camera
/// and reports "wrong password" when it gets `NotAuthorized`.
pub fn public(req: &Request) -> bool {
    matches!(
        (req.ns.as_str(), req.action.as_str()),
        (
            TDS,
            "GetSystemDateAndTime" | "GetServices" | "GetCapabilities" | "GetWsdlUrl"
        ) | (_, "GetServiceCapabilities")
            | (TRT, "CreateProfile")
    )
}

fn el(name: &str, inner: impl AsRef<str>) -> String {
    format!("<{name}>{}</{name}>", inner.as_ref())
}

fn text(name: &str, value: impl std::fmt::Display) -> String {
    el(name, escape(&value.to_string()))
}

fn response(action: &str, prefix: &str, inner: impl AsRef<str>) -> Reply {
    Reply::Ok(format!(
        "<{prefix}:{action}Response>{}</{prefix}:{action}Response>",
        inner.as_ref()
    ))
}

fn url(ctx: &Context, path: &str) -> String {
    format!("http://{}{path}", ctx.http_host)
}

fn range(name: &str, min: impl std::fmt::Display, max: impl std::fmt::Display) -> String {
    el(
        name,
        format!("{}{}", text("tt:Min", min), text("tt:Max", max)),
    )
}

fn utc_date_time(now: SystemTime) -> String {
    let u = time::Utc::of(now);
    el(
        "tt:UTCDateTime",
        format!(
            "{}{}",
            el(
                "tt:Time",
                format!(
                    "{}{}{}",
                    text("tt:Hour", u.hour),
                    text("tt:Minute", u.minute),
                    text("tt:Second", u.second)
                )
            ),
            el(
                "tt:Date",
                format!(
                    "{}{}{}",
                    text("tt:Year", u.year),
                    text("tt:Month", u.month),
                    text("tt:Day", u.day)
                )
            )
        ),
    )
}

pub fn handle(req: &Request, ctx: &Context, book: &mut PullPointBook) -> Reply {
    match req.ns.as_str() {
        TDS => device(req, ctx),
        TRT => media(req, ctx),
        TR2 => media2::handle(req, ctx),
        TPTZ => match ctx.camera.ptz() {
            Some(ptz) => ptz_service(req, ctx, ptz),
            None => Reply::Fault(Fault::action_not_supported()),
        },
        TEV | WSNT => event_service(req, ctx, book),
        TMD => device_io(req, ctx),
        TIMG => match ctx.camera.imaging() {
            Some(port) => imaging::handle(req, ctx, port),
            None => Reply::Fault(Fault::action_not_supported()),
        },
        _ => Reply::Fault(Fault::action_not_supported()),
    }
}

// ---------------------------------------------------------------- Device

fn services(ctx: &Context) -> Vec<(&'static str, &'static str, &'static str)> {
    let mut out = vec![
        (TDS, "/onvif/device_service", "tds"),
        (TRT, "/onvif/media_service", "trt"),
        (TR2, "/onvif/media2_service", "tr2"),
        (TEV, "/onvif/events_service", "tev"),
        (TMD, "/onvif/deviceio_service", "tmd"),
    ];
    if ctx.camera.ptz().is_some() {
        out.push((TPTZ, "/onvif/ptz_service", "tptz"));
    }
    if ctx.camera.imaging().is_some() {
        out.push((TIMG, "/onvif/imaging_service", "timg"));
    }
    out
}

fn device_service_capabilities(ctx: &Context) -> String {
    let logging = ctx.camera.keeps_log();
    format!(
        "<tds:Capabilities><tds:Network IPFilter=\"false\" ZeroConfiguration=\"false\" IPVersion6=\"false\" DynDNS=\"false\" NTP=\"0\"/>\
<tds:Security TLS1.0=\"false\" TLS1.1=\"false\" TLS1.2=\"false\" OnboardKeyGeneration=\"false\" AccessPolicyConfig=\"false\" DefaultAccessPolicy=\"false\" Dot1X=\"false\" RemoteUserHandling=\"false\" X.509Token=\"false\" SAMLToken=\"false\" KerberosToken=\"false\" UsernameToken=\"true\" HttpDigest=\"true\" RELToken=\"false\"/>\
<tds:System DiscoveryResolve=\"false\" DiscoveryBye=\"true\" RemoteDiscovery=\"false\" SystemBackup=\"false\" SystemLogging=\"{logging}\" FirmwareUpgrade=\"false\" HttpFirmwareUpgrade=\"false\" HttpSystemBackup=\"false\" HttpSystemLogging=\"false\" HttpSupportInformation=\"false\"/></tds:Capabilities>"
    )
}

fn media_service_capabilities(profiles: usize) -> String {
    format!(
        "<trt:Capabilities SnapshotUri=\"true\" Rotation=\"false\" VideoSourceMode=\"false\" OSD=\"false\" EXICompression=\"false\"><trt:ProfileCapabilities MaximumNumberOfProfiles=\"{profiles}\"/><trt:StreamingCapabilities RTPMulticast=\"false\" RTP_TCP=\"true\" RTP_RTSP_TCP=\"true\" NonAggregateControl=\"false\" NoRTSPStreaming=\"false\"/></trt:Capabilities>"
    )
}

/// Device IO: where recorders count audio outputs to offer talkback (Surveillance Station shows no talk
/// button for a camera whose only audio output is in the Media service).
fn device_io_service_capabilities(ctx: &Context) -> String {
    format!(
        "<tmd:Capabilities VideoSources=\"1\" VideoOutputs=\"0\" AudioSources=\"{}\" AudioOutputs=\"{}\" RelayOutputs=\"{}\" SerialPorts=\"0\" DigitalInputs=\"{}\" DigitalInputOptions=\"false\"/>",
        audio_sources(ctx),
        audio_outputs(ctx),
        ctx.camera.relay_outputs().len(),
        ctx.camera.digital_inputs().len()
    )
}

fn audio_sources(ctx: &Context) -> usize {
    usize::from(ctx.camera.profiles().iter().any(|p| p.audio))
}

fn audio_outputs(ctx: &Context) -> usize {
    usize::from(ctx.camera.audio_output())
}

fn relay_token_fault() -> Reply {
    Reply::Fault(Fault::invalid_arg("ter:RelayToken", "No such relay output"))
}

/// Every relay output is bistable and idles open (inactive): the one setting there is. Asking for the
/// same is accepted, anything else (monostable, an inverted idle state) is refused.
fn relay_settings(
    ctx: &Context,
    token: Option<&str>,
    props: Option<&Element>,
) -> Result<(), Fault> {
    if !token.is_some_and(|t| ctx.camera.relay_outputs().contains(&t)) {
        return Err(Fault::invalid_arg("ter:RelayToken", "No such relay output"));
    }
    let props = props.ok_or_else(|| Fault::invalid_arg("ter:ModeError", "No Properties"))?;
    if props.text_of("Mode") != Some("Bistable") {
        return Err(Fault::invalid_arg(
            "ter:ModeError",
            "Only the bistable mode is supported",
        ));
    }
    if props.text_of("IdleState") != Some("open") {
        return Err(Fault::invalid_arg(
            "ter:ModeError",
            "The idle state is open",
        ));
    }
    Ok(())
}

fn device_io(req: &Request, ctx: &Context) -> Reply {
    let action = req.action.as_str();
    let reply = |inner: String| response(action, "tmd", inner);
    let token = |n: usize, t: &str| {
        if n > 0 {
            text("tmd:Token", t)
        } else {
            String::new()
        }
    };
    match action {
        "GetServiceCapabilities" => reply(device_io_service_capabilities(ctx)),
        "GetVideoSources" => reply(text("tmd:Token", VIDEO_SOURCE)),
        "GetAudioSources" => reply(token(audio_sources(ctx), AUDIO_SOURCE)),
        "GetAudioOutputs" => reply(token(audio_outputs(ctx), AUDIO_OUTPUT)),
        "GetVideoOutputs" | "GetSerialPorts" => reply(String::new()),
        "GetRelayOutputOptions" => {
            let wanted = req.body.text_of("RelayOutputToken");
            let known = ctx.camera.relay_outputs();
            if wanted.is_some_and(|t| !known.contains(&t)) {
                return relay_token_fault();
            }
            reply(
                known
                    .iter()
                    .filter(|t| wanted.is_none_or(|w| w == **t))
                    .map(|t| {
                        format!(
                            "<tmd:RelayOutputOptions token=\"{t}\"><tmd:Mode>Bistable</tmd:Mode></tmd:RelayOutputOptions>"
                        )
                    })
                    .collect(),
            )
        }
        "SetRelayOutputSettings" => {
            let relay = req.body.child("RelayOutput");
            let token = relay.and_then(|r| r.attr("token"));
            relay_settings(ctx, token, relay.and_then(|r| r.child("Properties")))
                .map_or_else(Reply::Fault, |()| reply(String::new()))
        }
        "GetDigitalInputs" => reply(
            ctx.camera
                .digital_inputs()
                .iter()
                .map(|t| format!("<tmd:DigitalInputs token=\"{t}\"/>"))
                .collect(),
        ),
        _ => Reply::Fault(Fault::action_not_supported()),
    }
}

fn ptz_service_capabilities(features: PtzFeatures) -> String {
    format!(
        "<tptz:Capabilities EFlip=\"false\" Reverse=\"false\" GetCompatibleConfigurations=\"true\" MoveStatus=\"true\" StatusPosition=\"{}\"/>",
        features.position
    )
}

fn event_service_capabilities() -> String {
    format!(
        "<tev:Capabilities WSSubscriptionPolicySupport=\"false\" WSPullPointSupport=\"true\" WSPausableSubscriptionManagerInterfaceSupport=\"false\" MaxNotificationProducers=\"0\" MaxPullPoints=\"{}\" PersistentNotificationStorage=\"false\"/>",
        events::MAX_SUBSCRIPTIONS
    )
}

fn capabilities(ctx: &Context, category: &str) -> String {
    let all = category.is_empty() || category == "All";
    let want = |c: &str| all || category == c;
    let mut out = String::new();
    if want("Device") {
        let system = format!(
            "<tt:System><tt:DiscoveryResolve>false</tt:DiscoveryResolve><tt:DiscoveryBye>true</tt:DiscoveryBye><tt:RemoteDiscovery>false</tt:RemoteDiscovery><tt:SystemBackup>false</tt:SystemBackup><tt:SystemLogging>{}</tt:SystemLogging><tt:FirmwareUpgrade>false</tt:FirmwareUpgrade><tt:SupportedVersions><tt:Major>2</tt:Major><tt:Minor>5</tt:Minor></tt:SupportedVersions></tt:System>",
            ctx.camera.keeps_log()
        );
        out.push_str(&el(
            "tt:Device",
            format!(
                "{}{}{}{}{}",
                text("tt:XAddr", url(ctx, "/onvif/device_service")),
                "<tt:Network><tt:IPFilter>false</tt:IPFilter><tt:ZeroConfiguration>false</tt:ZeroConfiguration><tt:IPVersion6>false</tt:IPVersion6><tt:DynDNS>false</tt:DynDNS></tt:Network>",
                system,
                el(
                    "tt:IO",
                    format!(
                        "{}{}",
                        text("tt:InputConnectors", ctx.camera.digital_inputs().len()),
                        text("tt:RelayOutputs", ctx.camera.relay_outputs().len())
                    )
                ),
                "<tt:Security><tt:TLS1.1>false</tt:TLS1.1><tt:TLS1.2>false</tt:TLS1.2><tt:OnboardKeyGeneration>false</tt:OnboardKeyGeneration><tt:AccessPolicyConfig>false</tt:AccessPolicyConfig><tt:X.509Token>false</tt:X.509Token><tt:SAMLToken>false</tt:SAMLToken><tt:KerberosToken>false</tt:KerberosToken><tt:RELToken>false</tt:RELToken></tt:Security>",
            ),
        ));
    }
    if want("Events") {
        out.push_str(&el(
            "tt:Events",
            format!(
                "{}<tt:WSSubscriptionPolicySupport>false</tt:WSSubscriptionPolicySupport><tt:WSPullPointSupport>true</tt:WSPullPointSupport><tt:WSPausableSubscriptionManagerInterfaceSupport>false</tt:WSPausableSubscriptionManagerInterfaceSupport>",
                text("tt:XAddr", url(ctx, "/onvif/events_service"))
            ),
        ));
    }
    if want("Imaging") && ctx.camera.imaging().is_some() {
        out.push_str(&el(
            "tt:Imaging",
            text("tt:XAddr", url(ctx, "/onvif/imaging_service")),
        ));
    }
    if want("Media") {
        out.push_str(&el(
            "tt:Media",
            format!(
                "{}<tt:StreamingCapabilities><tt:RTPMulticast>false</tt:RTPMulticast><tt:RTP_TCP>true</tt:RTP_TCP><tt:RTP_RTSP_TCP>true</tt:RTP_RTSP_TCP></tt:StreamingCapabilities>",
                text("tt:XAddr", url(ctx, "/onvif/media_service"))
            ),
        ));
    }
    if want("PTZ") && ctx.camera.ptz().is_some() {
        out.push_str(&el(
            "tt:PTZ",
            text("tt:XAddr", url(ctx, "/onvif/ptz_service")),
        ));
    }
    if all {
        out.push_str(&el(
            "tt:Extension",
            el(
                "tt:DeviceIO",
                format!(
                    "{}{}{}{}{}{}",
                    text("tt:XAddr", url(ctx, "/onvif/deviceio_service")),
                    text("tt:VideoSources", 1),
                    text("tt:VideoOutputs", 0),
                    text("tt:AudioSources", audio_sources(ctx)),
                    text("tt:AudioOutputs", audio_outputs(ctx)),
                    text("tt:RelayOutputs", ctx.camera.relay_outputs().len())
                ),
            ),
        ));
    }
    out
}

/// What to quote to whoever supports this camera: the identity, one `key: value` per line.
fn support_information(ctx: &Context) -> String {
    let id = ctx.camera.identity();
    format!(
        "Manufacturer: {}\nModel: {}\nFirmware: {}\nHardware: {}\nSerial: {}\nMAC: {}\nHostname: {}\nONVIF: 2.5 (Profile S)\n",
        id.manufacturer,
        id.model,
        id.firmware,
        id.hardware,
        id.serial(),
        id.mac_text(),
        id.hostname,
    )
}

fn device(req: &Request, ctx: &Context) -> Reply {
    let id = ctx.camera.identity();
    let action = req.action.as_str();
    let reply = |inner: String| response(action, "tds", inner);
    match action {
        "GetDeviceInformation" => reply(format!(
            "{}{}{}{}{}",
            text("tds:Manufacturer", &id.manufacturer),
            text("tds:Model", &id.model),
            text("tds:FirmwareVersion", &id.firmware),
            text("tds:SerialNumber", id.serial()),
            text("tds:HardwareId", &id.hardware),
        )),
        "GetCapabilities" => {
            let category = req.body.text_of("Category").unwrap_or("All");
            let inner = capabilities(ctx, category);
            if inner.is_empty() {
                return Reply::Fault(Fault::receiver(
                    &["ter:ActionNotSupported", "ter:NoSuchService"],
                    "No such capability category",
                ));
            }
            reply(el("tds:Capabilities", inner))
        }
        "GetServices" => {
            let with_caps = req.body.text_of("IncludeCapability") == Some("true");
            let profiles = ctx.camera.profiles().len();
            let inner: String = services(ctx)
                .into_iter()
                .map(|(ns, path, prefix)| {
                    let caps = if with_caps {
                        let c = match prefix {
                            "tds" => device_service_capabilities(ctx),
                            "trt" => media_service_capabilities(profiles),
                            "tptz" => ptz_service_capabilities(
                                ctx.camera.ptz().map(|p| p.features()).unwrap_or_default(),
                            ),
                            "tmd" => device_io_service_capabilities(ctx),
                            "timg" => imaging::capabilities(),
                            "tr2" => media2::capabilities(ctx),
                            _ => event_service_capabilities(),
                        };
                        el("tds:Capabilities", c)
                    } else {
                        String::new()
                    };
                    el(
                        "tds:Service",
                        format!(
                            "{}{}{caps}<tds:Version><tt:Major>2</tt:Major><tt:Minor>5</tt:Minor></tds:Version>",
                            text("tds:Namespace", ns),
                            text("tds:XAddr", url(ctx, path)),
                        ),
                    )
                })
                .collect();
            reply(inner)
        }
        "GetServiceCapabilities" => reply(device_service_capabilities(ctx)),
        "GetSystemDateAndTime" => reply(el(
            "tds:SystemDateAndTime",
            format!(
                "<tt:DateTimeType>{}</tt:DateTimeType><tt:DaylightSavings>false</tt:DaylightSavings><tt:TimeZone><tt:TZ>UTC0</tt:TZ></tt:TimeZone>{}",
                // Truthful: NTP only when the camera follows a server.
                if id.ntp.is_empty() { "Manual" } else { "NTP" },
                utc_date_time(ctx.now)
            ),
        )),
        "GetScopes" => {
            let scopes = crate::discovery::scopes(id, ctx.camera.ptz().is_some());
            reply(
                scopes
                    .iter()
                    .map(|s| {
                        el(
                            "tds:Scopes",
                            format!(
                                "<tt:ScopeDef>Fixed</tt:ScopeDef>{}",
                                text("tt:ScopeItem", s)
                            ),
                        )
                    })
                    .collect(),
            )
        }
        "GetHostname" => reply(el(
            "tds:HostnameInformation",
            format!(
                "<tt:FromDHCP>false</tt:FromDHCP>{}",
                text("tt:Name", &id.hostname)
            ),
        )),
        "GetNetworkInterfaces" => {
            let address = ctx.host.trim_start_matches('[').trim_end_matches(']');
            let config = el(
                if id.dhcp { "tt:FromDHCP" } else { "tt:Manual" },
                format!(
                    "{}{}",
                    text("tt:Address", address),
                    text("tt:PrefixLength", id.prefix_len)
                ),
            );
            reply(format!(
                "<tds:NetworkInterfaces token=\"{}\"><tt:Enabled>true</tt:Enabled>{}{}</tds:NetworkInterfaces>",
                escape(&id.interface),
                el(
                    "tt:Info",
                    format!(
                        "{}{}<tt:MTU>1500</tt:MTU>",
                        text("tt:Name", &id.interface),
                        text("tt:HwAddress", id.mac_text())
                    )
                ),
                el(
                    "tt:IPv4",
                    format!(
                        "<tt:Enabled>true</tt:Enabled>{}",
                        el("tt:Config", format!("{config}{}", text("tt:DHCP", id.dhcp)))
                    )
                ),
            ))
        }
        "GetDNS" => reply(el(
            "tds:DNSInformation",
            format!(
                "{}{}",
                text("tt:FromDHCP", id.dhcp),
                id.dns
                    .iter()
                    .map(|d| {
                        el(
                            if id.dhcp {
                                "tt:DNSFromDHCP"
                            } else {
                                "tt:DNSManual"
                            },
                            format!("<tt:Type>IPv4</tt:Type>{}", text("tt:IPv4Address", d)),
                        )
                    })
                    .collect::<String>()
            ),
        )),
        "GetNTP" => reply(el(
            "tds:NTPInformation",
            format!(
                "<tt:FromDHCP>false</tt:FromDHCP>{}",
                id.ntp
                    .iter()
                    .map(|n| {
                        el(
                            "tt:NTPManual",
                            match n.parse::<std::net::IpAddr>() {
                                Ok(std::net::IpAddr::V4(_)) => {
                                    format!("<tt:Type>IPv4</tt:Type>{}", text("tt:IPv4Address", n))
                                }
                                Ok(std::net::IpAddr::V6(_)) => {
                                    format!("<tt:Type>IPv6</tt:Type>{}", text("tt:IPv6Address", n))
                                }
                                Err(_) => {
                                    format!("<tt:Type>DNS</tt:Type>{}", text("tt:DNSname", n))
                                }
                            },
                        )
                    })
                    .collect::<String>()
            ),
        )),
        "GetNetworkProtocols" => reply(format!(
            "<tds:NetworkProtocols><tt:Name>HTTP</tt:Name><tt:Enabled>true</tt:Enabled>{}</tds:NetworkProtocols><tds:NetworkProtocols><tt:Name>RTSP</tt:Name><tt:Enabled>true</tt:Enabled>{}</tds:NetworkProtocols>",
            text("tt:Port", ctx.http_port),
            text("tt:Port", ctx.rtsp_port)
        )),
        "GetDiscoveryMode" => reply("<tds:DiscoveryMode>Discoverable</tds:DiscoveryMode>".into()),
        "GetWsdlUrl" => reply(
            "<tds:WsdlUrl>http://www.onvif.org/ver10/device/wsdl/devicemgmt.wsdl</tds:WsdlUrl>"
                .into(),
        ),
        "GetRelayOutputs" => reply(
            ctx.camera
                .relay_outputs()
                .iter()
                .map(|t| {
                    format!(
                        "<tds:RelayOutputs token=\"{t}\"><tt:Properties><tt:Mode>Bistable</tt:Mode><tt:IdleState>open</tt:IdleState></tt:Properties></tds:RelayOutputs>"
                    )
                })
                .collect(),
        ),
        "SetRelayOutputState" => {
            let token = req.body.text_of("RelayOutputToken").unwrap_or_default();
            let active = match req.body.text_of("LogicalState") {
                Some("active") => true,
                Some("inactive") => false,
                _ => {
                    return Reply::Fault(Fault::invalid_arg(
                        "ter:InvalidArgs",
                        "LogicalState is active or inactive",
                    ));
                }
            };
            match ctx.camera.set_relay_output(token, active) {
                Ok(()) => reply(String::new()),
                Err(RelayError::NoSuchRelay) => relay_token_fault(),
                Err(RelayError::Failed(why)) => Reply::Fault(Fault::receiver(&["ter:Action"], why)),
            }
        }
        "SetRelayOutputSettings" => relay_settings(
            ctx,
            req.body.text_of("RelayOutputToken"),
            req.body.child("Properties"),
        )
        .map_or_else(Reply::Fault, |()| reply(String::new())),
        "GetUsers" => reply(format!(
            "<tds:User>{}{}</tds:User>",
            text("tt:Username", ctx.user),
            text("tt:UserLevel", "Administrator")
        )),
        "SystemReboot" => match ctx.camera.reboot() {
            Some(Ok(())) => reply(text("tds:Message", "Rebooting")),
            Some(Err(why)) => Reply::Fault(Fault::receiver(&["ter:Action"], why)),
            None => Reply::Fault(Fault::action_not_supported()),
        },
        "GetSystemLog" => match (req.body.text_of("LogType"), ctx.camera.system_log()) {
            (Some("System"), Some(log)) => reply(el("tds:SystemLog", text("tt:String", log))),
            (Some("System" | "Access"), _) => Reply::Fault(Fault::invalid_arg(
                "ter:SystemlogUnavailable",
                "The camera keeps no such log",
            )),
            _ => Reply::Fault(Fault::invalid_arg(
                "ter:InvalidArgs",
                "LogType is System or Access",
            )),
        },
        "GetSystemSupportInformation" => reply(el(
            "tds:SupportInformation",
            text("tt:String", support_information(ctx)),
        )),
        _ => Reply::Fault(Fault::action_not_supported()),
    }
}

// ---------------------------------------------------------------- Media

fn multicast() -> &'static str {
    "<tt:Multicast><tt:Address><tt:Type>IPv4</tt:Type><tt:IPv4Address>0.0.0.0</tt:IPv4Address></tt:Address><tt:Port>0</tt:Port><tt:TTL>1</tt:TTL><tt:AutoStart>false</tt:AutoStart></tt:Multicast>"
}

fn video_encoder_token(p: &MediaProfile) -> String {
    format!("{}_video", p.token)
}

fn source_size(ctx: &Context) -> (u32, u32, u32) {
    ctx.camera
        .profiles()
        .first()
        .map_or((0, 0, 0), |p| (p.video.width, p.video.height, p.video.fps))
}

fn video_source_config(ctx: &Context, element: &str, uses: usize) -> String {
    let (w, h, _) = source_size(ctx);
    format!(
        "<{element} token=\"{VIDEO_SOURCE_CONFIG}\">{}{}{}<tt:Bounds x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\"/></{element}>",
        text("tt:Name", VIDEO_SOURCE_CONFIG),
        text("tt:UseCount", uses),
        text("tt:SourceToken", VIDEO_SOURCE),
    )
}

fn audio_source_config(element: &str, uses: usize) -> String {
    format!(
        "<{element} token=\"{AUDIO_SOURCE_CONFIG}\">{}{}{}</{element}>",
        text("tt:Name", AUDIO_SOURCE_CONFIG),
        text("tt:UseCount", uses),
        text("tt:SourceToken", AUDIO_SOURCE),
    )
}

fn video_encoder_config(p: &MediaProfile, element: &str) -> String {
    let v = &p.video;
    format!(
        "<{element} token=\"{}\">{}<tt:UseCount>1</tt:UseCount><tt:Encoding>H264</tt:Encoding>{}<tt:Quality>5</tt:Quality>{}{}{}<tt:SessionTimeout>PT60S</tt:SessionTimeout></{element}>",
        video_encoder_token(p),
        text("tt:Name", video_encoder_token(p)),
        el(
            "tt:Resolution",
            format!(
                "{}{}",
                text("tt:Width", v.width),
                text("tt:Height", v.height)
            )
        ),
        el(
            "tt:RateControl",
            format!(
                "{}<tt:EncodingInterval>1</tt:EncodingInterval>{}",
                text("tt:FrameRateLimit", v.fps),
                text("tt:BitrateLimit", v.bitrate_kbps)
            )
        ),
        el(
            "tt:H264",
            format!(
                "{}{}",
                text("tt:GovLength", v.gop),
                text("tt:H264Profile", v.profile.onvif())
            )
        ),
        multicast(),
    )
}

fn audio_encoder_config(element: &str, uses: usize) -> String {
    format!(
        "<{element} token=\"{AUDIO_ENCODER}\">{}{}<tt:Encoding>G711</tt:Encoding><tt:Bitrate>64</tt:Bitrate><tt:SampleRate>8</tt:SampleRate>{}<tt:SessionTimeout>PT60S</tt:SessionTimeout></{element}>",
        text("tt:Name", AUDIO_ENCODER),
        text("tt:UseCount", uses),
        multicast(),
    )
}

fn ptz_config(element: &str, uses: usize, ptz: &dyn PtzPort) -> String {
    let (zoom, features) = (ptz.zoom(), ptz.features());
    let mut defaults = String::new();
    // The schema's order: absolute, relative, continuous, speed, timeout.
    if features.absolute {
        defaults.push_str(&text(
            "tt:DefaultAbsolutePantTiltPositionSpace",
            PAN_TILT_POSITION,
        ));
        if zoom {
            defaults.push_str(&text("tt:DefaultAbsoluteZoomPositionSpace", ZOOM_POSITION));
        }
    }
    if features.relative {
        defaults.push_str(&text(
            "tt:DefaultRelativePanTiltTranslationSpace",
            PAN_TILT_FOV,
        ));
        if zoom {
            defaults.push_str(&text(
                "tt:DefaultRelativeZoomTranslationSpace",
                ZOOM_TRANSLATION,
            ));
        }
    }
    defaults.push_str(&text(
        "tt:DefaultContinuousPanTiltVelocitySpace",
        PAN_TILT_VELOCITY,
    ));
    if zoom {
        defaults.push_str(&text(
            "tt:DefaultContinuousZoomVelocitySpace",
            ZOOM_VELOCITY,
        ));
    }
    if features.relative || features.absolute {
        // Clients read the default speed to fill their moves' `Speed`; a full-speed default.
        let zoom_speed = if zoom {
            format!("<tt:Zoom x=\"1\" space=\"{ZOOM_SPEED}\"/>")
        } else {
            String::new()
        };
        defaults.push_str(&format!(
            "<tt:DefaultPTZSpeed><tt:PanTilt x=\"1\" y=\"1\" space=\"{PAN_TILT_SPEED}\"/>{zoom_speed}</tt:DefaultPTZSpeed>"
        ));
    }
    format!(
        "<{element} token=\"{PTZ_CONFIG}\">{}{}{}{defaults}{}</{element}>",
        text("tt:Name", PTZ_CONFIG),
        text("tt:UseCount", uses),
        text("tt:NodeToken", PTZ_NODE),
        text("tt:DefaultPTZTimeout", time::format_duration(DEFAULT_MOVE)),
    )
}

fn audio_output_config(element: &str, uses: usize) -> String {
    format!(
        "<{element} token=\"{AUDIO_OUTPUT_CONFIG}\">{}{}{}<tt:OutputLevel>100</tt:OutputLevel></{element}>",
        text("tt:Name", AUDIO_OUTPUT_CONFIG),
        text("tt:UseCount", uses),
        text("tt:OutputToken", AUDIO_OUTPUT),
    )
}

fn audio_decoder_config(element: &str, uses: usize) -> String {
    format!(
        "<{element} token=\"{AUDIO_DECODER}\">{}{}</{element}>",
        text("tt:Name", AUDIO_DECODER),
        text("tt:UseCount", uses),
    )
}

fn profile_xml(ctx: &Context, p: &MediaProfile, element: &str) -> String {
    let uses = ctx.camera.profiles().len();
    let mut inner = text("tt:Name", p.token);
    inner.push_str(&video_source_config(
        ctx,
        "tt:VideoSourceConfiguration",
        uses,
    ));
    if p.audio {
        inner.push_str(&audio_source_config("tt:AudioSourceConfiguration", uses));
    }
    inner.push_str(&video_encoder_config(p, "tt:VideoEncoderConfiguration"));
    if p.audio {
        inner.push_str(&audio_encoder_config("tt:AudioEncoderConfiguration", uses));
    }
    if let Some(ptz) = ctx.camera.ptz() {
        inner.push_str(&ptz_config("tt:PTZConfiguration", uses, ptz));
    }
    if ctx.camera.audio_output() {
        inner.push_str(&el(
            "tt:Extension",
            format!(
                "{}{}",
                audio_output_config("tt:AudioOutputConfiguration", uses),
                audio_decoder_config("tt:AudioDecoderConfiguration", uses)
            ),
        ));
    }
    format!(
        "<{element} token=\"{}\" fixed=\"true\">{inner}</{element}>",
        p.token
    )
}

fn no_profile() -> Reply {
    Reply::Fault(Fault::invalid_arg("ter:NoProfile", "No such profile"))
}

fn no_config() -> Reply {
    Reply::Fault(Fault::invalid_arg("ter:NoConfig", "No such configuration"))
}

fn find_profile(ctx: &Context, req: &Request) -> Option<MediaProfile> {
    let token = req.body.text_of("ProfileToken")?;
    ctx.camera.profiles().into_iter().find(|p| p.token == token)
}

fn encoder_for(ctx: &Context, token: &str) -> Option<MediaProfile> {
    ctx.camera
        .profiles()
        .into_iter()
        .find(|p| video_encoder_token(p) == token)
}

/// The profile a `ConfigurationToken` or `ProfileToken` names; with neither, every profile.
fn encoder_scope(ctx: &Context, req: &Request) -> Result<Vec<MediaProfile>, Reply> {
    if let Some(token) = req.body.text_of("ConfigurationToken") {
        return encoder_for(ctx, token)
            .map(|p| vec![p])
            .ok_or_else(no_config);
    }
    if req.body.child("ProfileToken").is_some() {
        return find_profile(ctx, req)
            .map(|p| vec![p])
            .ok_or_else(no_profile);
    }
    Ok(ctx.camera.profiles())
}

fn number(e: &Element, path: &[&str]) -> Option<Option<u32>> {
    let mut at = e;
    for name in path {
        match at.child(name) {
            Some(next) => at = next,
            None => return Some(None),
        }
    }
    at.text.trim().parse().ok().map(Some)
}

/// `Some(true)` when every value the request sets equals the current one.
fn same_video(config: &Element, p: &MediaProfile) -> Option<bool> {
    let v = &p.video;
    let checks = [
        (number(config, &["Resolution", "Width"])?, v.width),
        (number(config, &["Resolution", "Height"])?, v.height),
        (number(config, &["RateControl", "FrameRateLimit"])?, v.fps),
        (
            number(config, &["RateControl", "BitrateLimit"])?,
            v.bitrate_kbps,
        ),
        (number(config, &["H264", "GovLength"])?, v.gop),
    ];
    let encoding = config.text_of("Encoding").is_none_or(|e| e == "H264");
    let profile = config
        .child("H264")
        .and_then(|h| h.text_of("H264Profile"))
        .is_none_or(|x| x == v.profile.onvif());
    Some(
        encoding
            && profile
            && checks
                .iter()
                .all(|(want, have)| want.is_none_or(|w| w == *have)),
    )
}

fn media(req: &Request, ctx: &Context) -> Reply {
    let action = req.action.as_str();
    let reply = |inner: String| response(action, "trt", inner);
    let profiles = ctx.camera.profiles();
    let uses = profiles.len();
    let (w, h, fps) = source_size(ctx);
    let config_token = req
        .body
        .child("Configuration")
        .and_then(|c| c.attr("token"))
        .or_else(|| req.body.text_of("ConfigurationToken"));
    match action {
        "GetServiceCapabilities" => reply(media_service_capabilities(uses)),
        "GetProfiles" => reply(
            profiles
                .iter()
                .map(|p| profile_xml(ctx, p, "trt:Profiles"))
                .collect(),
        ),
        "GetProfile" => match find_profile(ctx, req) {
            Some(p) => reply(profile_xml(ctx, &p, "trt:Profile")),
            None => no_profile(),
        },
        "CreateProfile" => Reply::Fault(Fault::receiver(
            &["ter:Action", "ter:MaxNVTProfiles"],
            "The camera has fixed profiles only",
        )),
        "DeleteProfile" => Reply::Fault(Fault::receiver(
            &["ter:Action", "ter:DeletionOfFixedProfile"],
            "Profiles are fixed",
        )),
        "GetVideoSources" => reply(format!(
            "<trt:VideoSources token=\"{VIDEO_SOURCE}\">{}{}</trt:VideoSources>",
            text("tt:Framerate", fps),
            el(
                "tt:Resolution",
                format!("{}{}", text("tt:Width", w), text("tt:Height", h))
            )
        )),
        "GetVideoSourceConfigurations" => {
            reply(video_source_config(ctx, "trt:Configurations", uses))
        }
        "GetVideoSourceConfiguration" => match config_token {
            Some(VIDEO_SOURCE_CONFIG) => reply(video_source_config(ctx, "trt:Configuration", uses)),
            _ => no_config(),
        },
        "GetVideoSourceConfigurationOptions" => reply(el(
            "trt:Options",
            format!(
                "{}{}",
                el(
                    "tt:BoundsRange",
                    format!(
                        "{}{}{}{}",
                        range("tt:XRange", 0, 0),
                        range("tt:YRange", 0, 0),
                        range("tt:WidthRange", w, w),
                        range("tt:HeightRange", h, h)
                    )
                ),
                text("tt:VideoSourceTokensAvailable", VIDEO_SOURCE)
            ),
        )),
        "GetVideoEncoderConfigurations" => reply(
            profiles
                .iter()
                .map(|p| video_encoder_config(p, "trt:Configurations"))
                .collect(),
        ),
        "GetVideoEncoderConfiguration" => match config_token.and_then(|t| encoder_for(ctx, t)) {
            Some(p) => reply(video_encoder_config(&p, "trt:Configuration")),
            None => no_config(),
        },
        "GetVideoEncoderConfigurationOptions" => match encoder_scope(ctx, req) {
            Ok(scope) => {
                let resolutions: String = scope
                    .iter()
                    .map(|p| {
                        el(
                            "tt:ResolutionsAvailable",
                            format!(
                                "{}{}",
                                text("tt:Width", p.video.width),
                                text("tt:Height", p.video.height)
                            ),
                        )
                    })
                    .collect();
                let span = |f: fn(&MediaProfile) -> u32| {
                    let values = scope.iter().map(f);
                    (values.clone().min().unwrap_or(0), values.max().unwrap_or(0))
                };
                let (gop_min, gop_max) = span(|p| p.video.gop);
                let (fps_min, fps_max) = span(|p| p.video.fps);
                let (kbps_min, kbps_max) = span(|p| p.video.bitrate_kbps);
                let mut kinds: Vec<&str> = scope.iter().map(|p| p.video.profile.onvif()).collect();
                kinds.dedup();
                let kinds: String = kinds
                    .iter()
                    .map(|k| text("tt:H264ProfilesSupported", k))
                    .collect();
                let h264 = format!(
                    "{resolutions}{}{}{}{kinds}",
                    range("tt:GovLengthRange", gop_min, gop_max),
                    range("tt:FrameRateRange", fps_min, fps_max),
                    range("tt:EncodingIntervalRange", 1, 1),
                );
                reply(el(
                    "trt:Options",
                    format!(
                        "{}{}{}",
                        range("tt:QualityRange", 0, 10),
                        el("tt:H264", &h264),
                        el(
                            "tt:Extension",
                            el(
                                "tt:H264",
                                format!("{h264}{}", range("tt:BitrateRange", kbps_min, kbps_max))
                            )
                        )
                    ),
                ))
            }
            Err(fault) => fault,
        },
        // The stream's key frames come every second, so a client that asks for one waits at most that long:
        // nothing needs forcing, and the request is valid for any profile of ours.
        "SetSynchronizationPoint" => match find_profile(ctx, req) {
            Some(_) => reply(String::new()),
            None => no_profile(),
        },
        // No metadata stream exists, so there is nothing to configure.
        "GetMetadataConfigurations" => reply(String::new()),
        "GetGuaranteedNumberOfVideoEncoderInstances" => reply(format!(
            "{}{}",
            text("trt:TotalNumber", uses),
            text("trt:H264", uses)
        )),
        "GetAudioSources" => reply(format!(
            "<trt:AudioSources token=\"{AUDIO_SOURCE}\"><tt:Channels>1</tt:Channels></trt:AudioSources>"
        )),
        "GetAudioSourceConfigurations" => reply(audio_source_config("trt:Configurations", uses)),
        "GetAudioSourceConfiguration" => match config_token {
            Some(AUDIO_SOURCE_CONFIG) => reply(audio_source_config("trt:Configuration", uses)),
            _ => no_config(),
        },
        "GetAudioSourceConfigurationOptions" => reply(el(
            "trt:Options",
            text("tt:InputTokensAvailable", AUDIO_SOURCE),
        )),
        "GetAudioEncoderConfigurations" => reply(audio_encoder_config("trt:Configurations", uses)),
        "GetAudioEncoderConfiguration" => match config_token {
            Some(AUDIO_ENCODER) => reply(audio_encoder_config("trt:Configuration", uses)),
            _ => no_config(),
        },
        "GetAudioEncoderConfigurationOptions" => reply(el(
            "trt:Options",
            "<tt:Options><tt:Encoding>G711</tt:Encoding><tt:BitrateList><tt:Items>64</tt:Items></tt:BitrateList><tt:SampleRateList><tt:Items>8</tt:Items></tt:SampleRateList></tt:Options>",
        )),
        "GetAudioOutputs" if ctx.camera.audio_output() => {
            reply(format!("<trt:AudioOutputs token=\"{AUDIO_OUTPUT}\"/>"))
        }
        "GetAudioOutputConfigurations" if ctx.camera.audio_output() => {
            reply(audio_output_config("trt:Configurations", uses))
        }
        "GetAudioOutputConfiguration" if ctx.camera.audio_output() => match config_token {
            Some(AUDIO_OUTPUT_CONFIG) => reply(audio_output_config("trt:Configuration", uses)),
            _ => no_config(),
        },
        "GetAudioOutputConfigurationOptions" if ctx.camera.audio_output() => reply(el(
            "trt:Options",
            format!(
                "{}<tt:SendPrimacyOptions>www.onvif.org/ver20/HalfDuplex/Auto</tt:SendPrimacyOptions>{}",
                text("tt:OutputTokensAvailable", AUDIO_OUTPUT),
                range("tt:OutputLevelRange", 100, 100)
            ),
        )),
        "GetAudioDecoderConfigurations" if ctx.camera.audio_output() => {
            reply(audio_decoder_config("trt:Configurations", uses))
        }
        "GetAudioDecoderConfiguration" if ctx.camera.audio_output() => match config_token {
            Some(AUDIO_DECODER) => reply(audio_decoder_config("trt:Configuration", uses)),
            _ => no_config(),
        },
        "GetAudioDecoderConfigurationOptions" if ctx.camera.audio_output() => reply(el(
            "trt:Options",
            "<tt:G711DecOptions><tt:Bitrate><tt:Items>64</tt:Items></tt:Bitrate><tt:SampleRateRange><tt:Items>8</tt:Items></tt:SampleRateRange></tt:G711DecOptions>",
        )),
        "GetStreamUri" => match find_profile(ctx, req) {
            Some(p) => reply(el(
                "trt:MediaUri",
                format!(
                    "{}<tt:InvalidAfterConnect>false</tt:InvalidAfterConnect><tt:InvalidAfterReboot>false</tt:InvalidAfterReboot><tt:Timeout>PT60S</tt:Timeout>",
                    text(
                        "tt:Uri",
                        format!("rtsp://{}:{}/{}", ctx.host, ctx.rtsp_port, p.stream_path)
                    )
                ),
            )),
            None => no_profile(),
        },
        "GetSnapshotUri" => match find_profile(ctx, req) {
            Some(_) => reply(el(
                "trt:MediaUri",
                format!(
                    "{}<tt:InvalidAfterConnect>false</tt:InvalidAfterConnect><tt:InvalidAfterReboot>false</tt:InvalidAfterReboot><tt:Timeout>PT0S</tt:Timeout>",
                    text("tt:Uri", url(ctx, "/snapshot.jpg"))
                ),
            )),
            None => no_profile(),
        },
        "SetVideoEncoderConfiguration" => {
            let Some(config) = req.body.child("Configuration") else {
                return no_config();
            };
            let Some(p) = config.attr("token").and_then(|t| encoder_for(ctx, t)) else {
                return no_config();
            };
            match same_video(config, &p) {
                Some(true) => reply(String::new()),
                _ => Reply::Fault(Fault::invalid_arg(
                    "ter:ConfigModify",
                    "Encoder settings are read-only over ONVIF",
                )),
            }
        }
        "SetAudioEncoderConfiguration" => {
            let Some(config) = req.body.child("Configuration") else {
                return no_config();
            };
            let same = config.attr("token") == Some(AUDIO_ENCODER)
                && config.text_of("Encoding").is_none_or(|e| e == "G711")
                && config.text_of("SampleRate").is_none_or(|r| r == "8");
            if same {
                reply(String::new())
            } else {
                Reply::Fault(Fault::invalid_arg(
                    "ter:ConfigModify",
                    "Audio is G.711 µ-law at 8 kHz",
                ))
            }
        }
        "SetVideoSourceConfiguration"
        | "SetAudioSourceConfiguration"
        | "SetAudioOutputConfiguration"
        | "SetAudioDecoderConfiguration" => {
            let known = [
                VIDEO_SOURCE_CONFIG,
                AUDIO_SOURCE_CONFIG,
                AUDIO_OUTPUT_CONFIG,
                AUDIO_DECODER,
            ];
            let config = req.body.child("Configuration");
            let token_ok = config
                .and_then(|c| c.attr("token"))
                .is_some_and(|t| known.contains(&t));
            let bounds_ok = config.and_then(|c| c.child("Bounds")).is_none_or(|b| {
                b.attr("width").is_none_or(|x| x == w.to_string())
                    && b.attr("height").is_none_or(|x| x == h.to_string())
            });
            if token_ok && bounds_ok {
                reply(String::new())
            } else {
                Reply::Fault(Fault::invalid_arg(
                    "ter:ConfigModify",
                    "Configurations are read-only over ONVIF",
                ))
            }
        }
        // A fixed profile already holds every configuration: re-adding its own is a no-op.
        a if a.starts_with("Add") && a.ends_with("Configuration") => {
            let Some(p) = find_profile(ctx, req) else {
                return no_profile();
            };
            let token = req.body.text_of("ConfigurationToken").unwrap_or_default();
            let mut held = vec![VIDEO_SOURCE_CONFIG.to_string(), video_encoder_token(&p)];
            if p.audio {
                held.extend([AUDIO_SOURCE_CONFIG.into(), AUDIO_ENCODER.into()]);
            }
            if ctx.camera.ptz().is_some() {
                held.push(PTZ_CONFIG.into());
            }
            if ctx.camera.audio_output() {
                held.extend([AUDIO_OUTPUT_CONFIG.into(), AUDIO_DECODER.into()]);
            }
            if held.iter().any(|t| t == token) {
                reply(String::new())
            } else {
                no_config()
            }
        }
        _ => Reply::Fault(Fault::action_not_supported()),
    }
}

// ---------------------------------------------------------------- PTZ

fn ptz_fault(e: PtzError) -> Reply {
    Reply::Fault(match e {
        PtzError::NoSuchPreset => Fault::invalid_arg("ter:NoToken", "No such preset"),
        PtzError::TooManyPresets => Fault::receiver(
            &["ter:Action", "ter:TooManyPresets"],
            "Preset store is full",
        ),
        PtzError::InvalidName => Fault::invalid_arg("ter:InvalidPresetName", "Invalid preset name"),
        PtzError::Unsupported => Fault::action_not_supported(),
        PtzError::Failed(why) => Fault::receiver(&["ter:Action"], why),
    })
}

fn velocity_space(name: &str, uri: &str, axes: &[&str]) -> String {
    el(
        name,
        format!(
            "{}{}",
            text("tt:URI", uri),
            axes.iter().map(|a| range(a, -1, 1)).collect::<String>()
        ),
    )
}

fn position_space(name: &str, uri: &str, axes: &[(&str, i32)]) -> String {
    el(
        name,
        format!(
            "{}{}",
            text("tt:URI", uri),
            axes.iter()
                .map(|(a, min)| range(a, min, 1))
                .collect::<String>()
        ),
    )
}

/// `SupportedPTZSpaces` / the options' `Spaces`, in the schema's order.
fn spaces(ptz: &dyn PtzPort) -> String {
    const XY: [(&str, i32); 2] = [("tt:XRange", -1), ("tt:YRange", -1)];
    let (zoom, features) = (ptz.zoom(), ptz.features());
    let mut out = String::new();
    if features.absolute {
        out.push_str(&position_space(
            "tt:AbsolutePanTiltPositionSpace",
            PAN_TILT_POSITION,
            &XY,
        ));
        if zoom {
            out.push_str(&position_space(
                "tt:AbsoluteZoomPositionSpace",
                ZOOM_POSITION,
                &[("tt:XRange", 0)],
            ));
        }
    }
    if features.relative {
        out.push_str(&position_space(
            "tt:RelativePanTiltTranslationSpace",
            PAN_TILT_FOV,
            &XY,
        ));
        if zoom {
            out.push_str(&position_space(
                "tt:RelativeZoomTranslationSpace",
                ZOOM_TRANSLATION,
                &[("tt:XRange", -1)],
            ));
        }
    }
    out.push_str(&velocity_space(
        "tt:ContinuousPanTiltVelocitySpace",
        PAN_TILT_VELOCITY,
        &["tt:XRange", "tt:YRange"],
    ));
    if zoom {
        out.push_str(&velocity_space(
            "tt:ContinuousZoomVelocitySpace",
            ZOOM_VELOCITY,
            &["tt:XRange"],
        ));
    }
    if features.relative || features.absolute {
        out.push_str(&position_space(
            "tt:PanTiltSpeedSpace",
            PAN_TILT_SPEED,
            &[("tt:XRange", 0)],
        ));
        if zoom {
            out.push_str(&position_space(
                "tt:ZoomSpeedSpace",
                ZOOM_SPEED,
                &[("tt:XRange", 0)],
            ));
        }
    }
    out
}

/// An axis value that must be present and finite (a move's target), clamped to `min..=1`.
fn target(e: &Element, name: &str, min: f32) -> Result<f32, Fault> {
    e.attr(name)
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .map(|v| v.clamp(min, 1.0))
        .ok_or_else(|| Fault::invalid_arg("ter:InvalidPosition", "A move needs finite x and y"))
}

/// The `Translation` / `Position` child `name` of a move: `None` when the axis is absent. A `space` it names
/// must be the one we offer for it.
fn move_axes(
    body: &Element,
    parent: &str,
    name: &str,
    uri: &str,
    min: f32,
    both: bool,
) -> Result<Option<(f32, f32)>, Fault> {
    let Some(e) = body.child(parent).and_then(|p| p.child(name)) else {
        return Ok(None);
    };
    if e.attr("space").is_some_and(|s| s != uri) {
        return Err(Fault::invalid_arg(
            "ter:SpaceNotSupported",
            "That coordinate space is not supported",
        ));
    }
    let x = target(e, "x", min)?;
    let y = if both { target(e, "y", min)? } else { 0.0 };
    Ok(Some((x, y)))
}

fn axis(e: Option<&Element>, name: &str) -> f32 {
    e.and_then(|e| e.attr(name))
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .map_or(0.0, |v| v.clamp(-1.0, 1.0))
}

fn ptz_service(req: &Request, ctx: &Context, ptz: &dyn PtzPort) -> Reply {
    let action = req.action.as_str();
    let reply = |inner: String| response(action, "tptz", inner);
    let uses = ctx.camera.profiles().len();
    let node = || {
        let aux: String = ptz
            .aux_commands()
            .iter()
            .map(|c| text("tt:AuxiliaryCommands", c))
            .collect();
        format!(
            "<tptz:PTZNode token=\"{PTZ_NODE}\" FixedHomePosition=\"{}\">{}{}{}<tt:HomeSupported>true</tt:HomeSupported>{aux}</tptz:PTZNode>",
            !ptz.features().set_home,
            text("tt:Name", "PTZ"),
            el("tt:SupportedPTZSpaces", spaces(ptz)),
            text("tt:MaximumNumberOfPresets", ptz.max_presets()),
        )
    };
    // Calls that name a profile must name one of ours.
    if req.body.child("ProfileToken").is_some() && find_profile(ctx, req).is_none() {
        return no_profile();
    }
    match action {
        "GetServiceCapabilities" => reply(ptz_service_capabilities(ptz.features())),
        "GetNodes" => reply(node()),
        "GetNode" => match req.body.text_of("NodeToken") {
            Some(PTZ_NODE) => reply(node()),
            _ => Reply::Fault(Fault::invalid_arg("ter:NoEntity", "No such PTZ node")),
        },
        "GetConfigurations" => reply(ptz_config("tptz:PTZConfiguration", uses, ptz)),
        "GetConfiguration" => match req.body.text_of("PTZConfigurationToken") {
            Some(PTZ_CONFIG) => reply(ptz_config("tptz:PTZConfiguration", uses, ptz)),
            _ => no_config(),
        },
        "GetConfigurationOptions" => reply(el(
            "tptz:PTZConfigurationOptions",
            format!(
                "{}{}",
                el("tt:Spaces", spaces(ptz)),
                el(
                    "tt:PTZTimeout",
                    format!(
                        "{}{}",
                        text("tt:Min", "PT1S"),
                        text("tt:Max", time::format_duration(MAX_MOVE))
                    )
                )
            ),
        )),
        "GetStatus" => {
            let status = if ptz.moving() { "MOVING" } else { "IDLE" };
            let zoom = if ptz.zoom() {
                text("tt:Zoom", status)
            } else {
                String::new()
            };
            // `Position` precedes `MoveStatus` in the schema; it is left out while the pose is unknown.
            let position = ptz
                .features()
                .position
                .then(|| ptz.pose())
                .flatten()
                .map(|p| {
                    let zoom = if ptz.zoom() {
                        format!("<tt:Zoom x=\"{:.4}\" space=\"{ZOOM_POSITION}\"/>", p.zoom)
                    } else {
                        String::new()
                    };
                    el(
                        "tt:Position",
                        format!(
                            "<tt:PanTilt x=\"{:.4}\" y=\"{:.4}\" space=\"{PAN_TILT_POSITION}\"/>{zoom}",
                            p.pan, p.tilt
                        ),
                    )
                })
                .unwrap_or_default();
            reply(el(
                "tptz:PTZStatus",
                format!(
                    "{position}{}{}",
                    el(
                        "tt:MoveStatus",
                        format!("{}{zoom}", text("tt:PanTilt", status))
                    ),
                    text("tt:UtcTime", time::format(ctx.now))
                ),
            ))
        }
        "RelativeMove" | "AbsoluteMove" => {
            let relative = action == "RelativeMove";
            let (parent, pt_uri, pt_min, zoom_uri, zoom_min) = if relative {
                ("Translation", PAN_TILT_FOV, -1.0, ZOOM_TRANSLATION, -1.0)
            } else {
                ("Position", PAN_TILT_POSITION, -1.0, ZOOM_POSITION, 0.0)
            };
            let features = ptz.features();
            if !(if relative {
                features.relative
            } else {
                features.absolute
            }) {
                return ptz_fault(PtzError::Unsupported);
            }
            let axes =
                move_axes(&req.body, parent, "PanTilt", pt_uri, pt_min, true).and_then(|pt| {
                    if ptz.zoom() {
                        move_axes(&req.body, parent, "Zoom", zoom_uri, zoom_min, false)
                            .map(|z| (pt, z.map(|(x, _)| x)))
                    } else {
                        Ok((pt, None))
                    }
                });
            let (pan_tilt, zoom) = match axes {
                Ok((None, None)) => {
                    return Reply::Fault(Fault::invalid_arg(
                        "ter:InvalidPosition",
                        "The move names no axis",
                    ));
                }
                Ok(axes) => axes,
                Err(f) => return Reply::Fault(f),
            };
            // `Speed` is not honoured: a correction is always made as fast as the head can.
            let result = if relative {
                ptz.relative_move(Relative { pan_tilt, zoom })
            } else {
                ptz.absolute_move(Absolute { pan_tilt, zoom })
            };
            match result {
                Ok(()) => Reply::Stopped(format!("<tptz:{action}Response/>")),
                Err(e) => ptz_fault(e),
            }
        }
        "ContinuousMove" => {
            let velocity = req.body.child("Velocity");
            let v = Velocity {
                pan: axis(velocity.and_then(|v| v.child("PanTilt")), "x"),
                tilt: axis(velocity.and_then(|v| v.child("PanTilt")), "y"),
                zoom: if ptz.zoom() {
                    axis(velocity.and_then(|v| v.child("Zoom")), "x")
                } else {
                    0.0
                },
            };
            let timeout = match req.body.text_of("Timeout") {
                Some(t) => match time::parse_duration(t) {
                    Some(d) => d.min(MAX_MOVE),
                    None => {
                        return Reply::Fault(Fault::invalid_arg("ter:InvalidArgs", "Bad Timeout"));
                    }
                },
                None => DEFAULT_MOVE,
            };
            match ptz.continuous_move(v) {
                Ok(()) => Reply::Moving {
                    body: "<tptz:ContinuousMoveResponse/>".into(),
                    stop_after: timeout,
                },
                Err(e) => ptz_fault(e),
            }
        }
        "Stop" => {
            // No flag stops everything. The head cannot stop one axis alone, so a flag naming one axis
            // stops both (a safe superset); only an explicit "neither" leaves the camera moving.
            let flag = |name| req.body.text_of(name).map(|v| v == "true" || v == "1");
            if flag("PanTilt") == Some(false) && flag("Zoom") == Some(false) {
                return Reply::Ok("<tptz:StopResponse/>".into());
            }
            match ptz.stop() {
                Ok(()) => Reply::Stopped("<tptz:StopResponse/>".into()),
                Err(e) => ptz_fault(e),
            }
        }
        "SetHomePosition" => match ptz.set_home() {
            Ok(()) => reply(String::new()),
            Err(e) => ptz_fault(e),
        },
        "SendAuxiliaryCommand" => {
            let command = req.body.text_of("AuxiliaryData").unwrap_or_default();
            if !ptz.aux_commands().contains(&command) {
                return Reply::Fault(Fault::invalid_arg(
                    "ter:InvalidArgs",
                    "That auxiliary command is not offered",
                ));
            }
            match ptz.aux_command(command) {
                Ok(done) => reply(text("tptz:AuxiliaryResponse", done)),
                Err(e) => ptz_fault(e),
            }
        }
        "GetCompatibleConfigurations" => reply(ptz_config("tptz:PTZConfiguration", uses, ptz)),
        "GotoHomePosition" => match ptz.home() {
            Ok(()) => Reply::Stopped("<tptz:GotoHomePositionResponse/>".into()),
            Err(e) => ptz_fault(e),
        },
        "GetPresets" => reply(
            ptz.presets()
                .iter()
                .map(|p| {
                    format!(
                        "<tptz:Preset token=\"{}\">{}</tptz:Preset>",
                        escape(&p.token),
                        text("tt:Name", &p.name)
                    )
                })
                .collect(),
        ),
        "GotoPreset" => match req.body.text_of("PresetToken") {
            Some(token) => match ptz.goto_preset(token) {
                Ok(()) => Reply::Stopped("<tptz:GotoPresetResponse/>".into()),
                Err(e) => ptz_fault(e),
            },
            None => ptz_fault(PtzError::NoSuchPreset),
        },
        "SetPreset" => {
            let token = req.body.text_of("PresetToken").filter(|t| !t.is_empty());
            let name = req.body.text_of("PresetName").filter(|n| !n.is_empty());
            match ptz.set_preset(token, name) {
                Ok(token) => reply(text("tptz:PresetToken", token)),
                Err(e) => ptz_fault(e),
            }
        }
        "RemovePreset" => match req.body.text_of("PresetToken") {
            Some(token) => match ptz.remove_preset(token) {
                Ok(()) => reply(String::new()),
                Err(e) => ptz_fault(e),
            },
            None => ptz_fault(PtzError::NoSuchPreset),
        },
        _ => Reply::Fault(Fault::action_not_supported()),
    }
}

// ---------------------------------------------------------------- Events

fn event_fault(e: EventError) -> Reply {
    Reply::Fault(match e {
        EventError::TooManySubscriptions => Fault::receiver(
            &["ter:Action", "ter:TooManySubscriptions"],
            "No more pull points",
        ),
        EventError::NoSuchSubscription => Fault::sender(
            &["ter:InvalidArgVal", "ter:NoSubscription"],
            "No such subscription",
        ),
    })
}

/// The subscription a request addresses: its URL path, else its WS-Addressing `To`.
fn subscription(req: &Request, ctx: &Context) -> Option<u32> {
    ctx.subscription.or_else(|| {
        req.to
            .as_deref()?
            .rsplit_once("/onvif/subscription/")?
            .1
            .parse()
            .ok()
    })
}

fn termination(e: &Element, name: &str) -> Result<Option<Termination>, Reply> {
    match e.text_of(name).filter(|t| !t.is_empty()) {
        None => Ok(None),
        Some(t) => Termination::parse(t).map(Some).ok_or_else(|| {
            Reply::Fault(Fault::invalid_arg(
                "ter:InvalidArgs",
                "Bad termination time",
            ))
        }),
    }
}

fn topic_set(inputs: &[&str]) -> String {
    if inputs.is_empty() {
        return "<wstop:TopicSet/>".into();
    }
    "<wstop:TopicSet><tns1:Device><tns1:Trigger><tns1:DigitalInput wstop:topic=\"true\"><tt:MessageDescription IsProperty=\"true\"><tt:Source><tt:SimpleItemDescription Name=\"InputToken\" Type=\"tt:ReferenceToken\"/></tt:Source><tt:Data><tt:SimpleItemDescription Name=\"LogicalState\" Type=\"xs:boolean\"/></tt:Data></tt:MessageDescription></tns1:DigitalInput></tns1:Trigger></tns1:Device></wstop:TopicSet>".into()
}

fn event_service(req: &Request, ctx: &Context, book: &mut PullPointBook) -> Reply {
    let action = req.action.as_str();
    let now = ctx.now;
    match (req.ns.as_str(), action) {
        (TEV, "GetServiceCapabilities") => response(action, "tev", event_service_capabilities()),
        (TEV, "GetEventProperties") => response(
            action,
            "tev",
            format!(
                "<tev:TopicNamespaceLocation>http://www.onvif.org/onvif/ver10/topics/topicns.xml</tev:TopicNamespaceLocation><wsnt:FixedTopicSet>true</wsnt:FixedTopicSet>{}<wsnt:TopicExpressionDialect>http://www.onvif.org/ver10/tev/topicExpression/ConcreteSet</wsnt:TopicExpressionDialect><wsnt:TopicExpressionDialect>http://docs.oasis-open.org/wsn/t-1/TopicExpression/Concrete</wsnt:TopicExpressionDialect><tev:MessageContentFilterDialect>http://www.onvif.org/ver10/tev/messageContentFilter/ItemFilter</tev:MessageContentFilterDialect><tev:MessageContentSchemaLocation>http://www.onvif.org/onvif/ver10/schema/onvif.xsd</tev:MessageContentSchemaLocation>",
                topic_set(ctx.camera.digital_inputs())
            ),
        ),
        (TEV, "CreatePullPointSubscription") => {
            let requested = match termination(&req.body, "InitialTerminationTime") {
                Ok(t) => t,
                Err(fault) => return fault,
            };
            match book.create(now, requested) {
                Ok((id, expires)) => response(
                    action,
                    "tev",
                    format!(
                        "<tev:SubscriptionReference>{}</tev:SubscriptionReference>{}{}",
                        text(
                            "wsa:Address",
                            url(ctx, &format!("/onvif/subscription/{id}"))
                        ),
                        text("wsnt:CurrentTime", time::format(now)),
                        text("wsnt:TerminationTime", time::format(expires))
                    ),
                ),
                Err(e) => event_fault(e),
            }
        }
        (TEV, "PullMessages") => {
            let Some(id) = subscription(req, ctx) else {
                return event_fault(EventError::NoSuchSubscription);
            };
            if let Err(e) = book.pull(id, now, 0) {
                return event_fault(e);
            }
            let wait = req
                .body
                .text_of("Timeout")
                .and_then(time::parse_duration)
                .unwrap_or(MAX_PULL_WAIT)
                .min(MAX_PULL_WAIT);
            let limit = req
                .body
                .text_of("MessageLimit")
                .and_then(|l| l.parse::<usize>().ok())
                .unwrap_or(MAX_PULL_MESSAGES)
                .clamp(1, MAX_PULL_MESSAGES);
            Reply::Pull { id, wait, limit }
        }
        (TEV, "SetSynchronizationPoint") => match subscription(req, ctx) {
            Some(id) => match book.synchronize(id, now) {
                Ok(()) => response(action, "tev", ""),
                Err(e) => event_fault(e),
            },
            None => event_fault(EventError::NoSuchSubscription),
        },
        (WSNT, "Renew") => {
            let requested = match termination(&req.body, "TerminationTime") {
                Ok(t) => t,
                Err(fault) => return fault,
            };
            match subscription(req, ctx).map(|id| book.renew(id, now, requested)) {
                Some(Ok(expires)) => response(
                    action,
                    "wsnt",
                    format!(
                        "{}{}",
                        text("wsnt:TerminationTime", time::format(expires)),
                        text("wsnt:CurrentTime", time::format(now))
                    ),
                ),
                Some(Err(e)) => event_fault(e),
                None => event_fault(EventError::NoSuchSubscription),
            }
        }
        (WSNT, "Unsubscribe") => match subscription(req, ctx).map(|id| book.unsubscribe(id, now)) {
            Some(Ok(())) => response(action, "wsnt", ""),
            Some(Err(e)) => event_fault(e),
            None => event_fault(EventError::NoSuchSubscription),
        },
        _ => Reply::Fault(Fault::action_not_supported()),
    }
}

/// The PullMessages answer once the adapter has waited.
pub fn pull_response(now: SystemTime, expires: SystemTime, messages: &[Message]) -> String {
    let notifications: String = messages
        .iter()
        .map(|m| {
            format!(
                "<wsnt:NotificationMessage><wsnt:Topic Dialect=\"http://www.onvif.org/ver10/tev/topicExpression/ConcreteSet\">{}</wsnt:Topic><wsnt:Message><tt:Message UtcTime=\"{}\" PropertyOperation=\"{}\"><tt:Source><tt:SimpleItem Name=\"InputToken\" Value=\"{}\"/></tt:Source><tt:Data><tt:SimpleItem Name=\"LogicalState\" Value=\"{}\"/></tt:Data></tt:Message></wsnt:Message></wsnt:NotificationMessage>",
                events::TOPIC,
                time::format(m.time),
                m.operation.onvif(),
                escape(&m.input),
                m.state
            )
        })
        .collect();
    format!(
        "<tev:PullMessagesResponse>{}{}{notifications}</tev:PullMessagesResponse>",
        text("tev:CurrentTime", time::format(now)),
        text("tev:TerminationTime", time::format(expires))
    )
}

mod imaging;
mod media2;

#[cfg(test)]
mod tests;
