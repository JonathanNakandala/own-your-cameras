//! The Media2 service (`ver20/media/wsdl`, Profile T's media): the same two fixed profiles and the same
//! configurations as [`super::media`], in Media2's shapes. The differences that matter to a client:
//!
//! * `GetProfiles` returns a profile's configurations only for the `Type`s asked for (`All` for every one);
//!   with no `Type` it returns just the token and name.
//! * The video encoder's `GovLength` and `Profile` are attributes, and codecs are named as RFC 3551 does
//!   (`H264`, `PCMU`).
//! * `GetStreamUri` returns the bare `Uri`, for `RtspUnicast` or `RTSP`.
//!
//! Like Media1 it is read-only: a `Set…Configuration` that changes nothing succeeds, one that would change
//! something is refused.

use super::{
    AUDIO_DECODER, AUDIO_ENCODER, AUDIO_OUTPUT, AUDIO_OUTPUT_CONFIG, AUDIO_SOURCE_CONFIG, Context,
    PTZ_CONFIG, Reply, VIDEO_SOURCE_CONFIG, audio_decoder_config, audio_output_config,
    audio_source_config, el, encoder_for, encoder_scope, find_profile, multicast, no_config,
    no_profile, number, ptz_config, range, response, source_size, text, url, video_encoder_token,
    video_source_config,
};
use crate::camera::MediaProfile;
use crate::soap::{Fault, Request};
use crate::xml::Element;

/// `Type` values of `GetProfiles`, in the order a profile lists its configurations.
const TYPES: [&str; 8] = [
    "VideoSource",
    "AudioSource",
    "VideoEncoder",
    "AudioEncoder",
    "PTZ",
    "AudioOutput",
    "AudioDecoder",
    "Metadata",
];

pub(super) fn capabilities(ctx: &Context) -> String {
    let mut supported = vec!["VideoSource", "VideoEncoder"];
    if ctx.camera.profiles().iter().any(|p| p.audio) {
        supported.extend(["AudioSource", "AudioEncoder"]);
    }
    if ctx.camera.ptz().is_some() {
        supported.push("PTZ");
    }
    if ctx.camera.audio_output() {
        supported.extend(["AudioOutput", "AudioDecoder"]);
    }
    format!(
        "<tr2:Capabilities SnapshotUri=\"true\" Rotation=\"false\" VideoSourceMode=\"false\" OSD=\"false\" TemporaryOSDText=\"false\" Mask=\"false\" SourceMask=\"false\"><tr2:ProfileCapabilities MaximumNumberOfProfiles=\"{}\" ConfigurationsSupported=\"{}\"/><tr2:StreamingCapabilities RTSPStreaming=\"true\" RTPMulticast=\"false\" RTP_RTSP_TCP=\"true\" NonAggregateControl=\"false\" AutoStartMulticast=\"false\" SecureRTSPStreaming=\"false\"/></tr2:Capabilities>",
        ctx.camera.profiles().len(),
        supported.join(" ")
    )
}

/// `VideoEncoder2Configuration`: the codec, size and rate as elements, `GovLength` and `Profile` as attributes.
fn video_encoder(p: &MediaProfile, element: &str) -> String {
    let v = &p.video;
    format!(
        "<{element} token=\"{token}\" GovLength=\"{}\" Profile=\"{}\">{}<tt:UseCount>1</tt:UseCount><tt:Encoding>H264</tt:Encoding>{}{}{}<tt:Quality>5</tt:Quality></{element}>",
        v.gop,
        v.profile.onvif(),
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
                "{}{}",
                text("tt:FrameRateLimit", v.fps),
                text("tt:BitrateLimit", v.bitrate_kbps)
            )
        ),
        multicast(),
        token = video_encoder_token(p),
    )
}

/// `AudioEncoder2Configuration`: G.711 µ-law at 8 kHz, which is `PCMU` in Media2.
fn audio_encoder(element: &str, uses: usize) -> String {
    format!(
        "<{element} token=\"{AUDIO_ENCODER}\">{}{}<tt:Encoding>PCMU</tt:Encoding>{}<tt:Bitrate>64</tt:Bitrate><tt:SampleRate>8</tt:SampleRate></{element}>",
        text("tt:Name", AUDIO_ENCODER),
        text("tt:UseCount", uses),
        multicast(),
    )
}

/// One profile: its token and name, and the configurations of the `wanted` types.
fn profile(ctx: &Context, p: &MediaProfile, wanted: &[&str]) -> String {
    let uses = ctx.camera.profiles().len();
    let has = |t: &str| wanted.contains(&t);
    let mut set = String::new();
    if has("VideoSource") {
        set.push_str(&video_source_config(ctx, "tr2:VideoSource", uses));
    }
    if has("AudioSource") && p.audio {
        set.push_str(&audio_source_config("tr2:AudioSource", uses));
    }
    if has("VideoEncoder") {
        set.push_str(&video_encoder(p, "tr2:VideoEncoder"));
    }
    if has("AudioEncoder") && p.audio {
        set.push_str(&audio_encoder("tr2:AudioEncoder", uses));
    }
    if has("PTZ")
        && let Some(ptz) = ctx.camera.ptz()
    {
        set.push_str(&ptz_config("tr2:PTZ", uses, ptz));
    }
    if ctx.camera.audio_output() {
        if has("AudioOutput") {
            set.push_str(&audio_output_config("tr2:AudioOutput", uses));
        }
        if has("AudioDecoder") {
            set.push_str(&audio_decoder_config("tr2:AudioDecoder", uses));
        }
    }
    let configurations = if set.is_empty() {
        String::new()
    } else {
        el("tr2:Configurations", set)
    };
    format!(
        "<tr2:Profiles token=\"{}\" fixed=\"true\">{}{configurations}</tr2:Profiles>",
        p.token,
        text("tr2:Name", p.token)
    )
}

/// The types a `GetProfiles` asked for: none, a list, or `All`.
fn wanted(req: &Request) -> Vec<&'static str> {
    let asked: Vec<&str> = req
        .body
        .children_named("Type")
        .map(|t| t.text.trim())
        .collect();
    if asked.contains(&"All") {
        return TYPES.to_vec();
    }
    TYPES.into_iter().filter(|t| asked.contains(t)).collect()
}

/// The shared answer of the single-configuration getters: a `ConfigurationToken` must name ours, a
/// `ProfileToken` one of our profiles; with neither (or both right) the configuration is returned.
fn single(ctx: &Context, req: &Request, token: &str, build: impl FnOnce() -> String) -> Reply {
    if req
        .body
        .text_of("ConfigurationToken")
        .is_some_and(|t| t != token)
    {
        return no_config();
    }
    if req.body.child("ProfileToken").is_some() && find_profile(ctx, req).is_none() {
        return no_profile();
    }
    response(req.action.as_str(), "tr2", build())
}

/// The numbers a `Set…Configuration` of the video encoder would change, as `Some(true)` when none differs.
fn same_encoder(config: &Element, p: &MediaProfile) -> Option<bool> {
    let v = &p.video;
    let attr = |name: &str| config.attr(name).map(|x| x.trim().parse::<u32>().ok());
    let checks = [
        (number(config, &["Resolution", "Width"])?, v.width),
        (number(config, &["Resolution", "Height"])?, v.height),
        (
            number(config, &["RateControl", "BitrateLimit"])?,
            v.bitrate_kbps,
        ),
    ];
    // The frame rate is a float in Media2 (15 or 15.0).
    let fps = match config
        .child("RateControl")
        .and_then(|r| r.text_of("FrameRateLimit"))
    {
        Some(f) => Some(f.trim().parse::<f32>().ok()?),
        None => None,
    };
    let gop = match attr("GovLength") {
        Some(g) => Some(g?),
        None => None,
    };
    Some(
        config.text_of("Encoding").is_none_or(|e| e == "H264")
            && config
                .attr("Profile")
                .is_none_or(|x| x == v.profile.onvif())
            && gop.is_none_or(|g| g == v.gop)
            && fps.is_none_or(|f| f == v.fps as f32)
            && checks
                .iter()
                .all(|(want, have)| want.is_none_or(|w| w == *have)),
    )
}

pub(super) fn handle(req: &Request, ctx: &Context) -> Reply {
    let action = req.action.as_str();
    let reply = |inner: String| response(action, "tr2", inner);
    let profiles = ctx.camera.profiles();
    let uses = profiles.len();
    let (w, h, _) = source_size(ctx);
    match action {
        "GetServiceCapabilities" => reply(capabilities(ctx)),
        "GetProfiles" => {
            let asked = wanted(req);
            match req.body.text_of("Token") {
                Some(token) => match profiles.iter().find(|p| p.token == token) {
                    Some(p) => reply(profile(ctx, p, &asked)),
                    None => no_profile(),
                },
                None => reply(profiles.iter().map(|p| profile(ctx, p, &asked)).collect()),
            }
        }
        "CreateProfile" => Reply::Fault(Fault::receiver(
            &["ter:Action", "ter:MaxNVTProfiles"],
            "The camera has fixed profiles only",
        )),
        "DeleteProfile" => Reply::Fault(Fault::receiver(
            &["ter:Action", "ter:DeletionOfFixedProfile"],
            "Profiles are fixed",
        )),
        "GetVideoSourceConfigurations" => single(ctx, req, VIDEO_SOURCE_CONFIG, || {
            video_source_config(ctx, "tr2:Configurations", uses)
        }),
        "GetVideoSourceConfigurationOptions" => reply(el(
            "tr2:Options",
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
                text("tt:VideoSourceTokensAvailable", super::VIDEO_SOURCE)
            ),
        )),
        "GetVideoEncoderConfigurations" => match encoder_scope(ctx, req) {
            Ok(scope) => reply(
                scope
                    .iter()
                    .map(|p| video_encoder(p, "tr2:Configurations"))
                    .collect(),
            ),
            Err(fault) => fault,
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
                let values = |f: fn(&MediaProfile) -> u32| {
                    let mut all: Vec<u32> = scope.iter().map(f).collect();
                    all.sort_unstable();
                    all.dedup();
                    all
                };
                let list = |v: Vec<u32>| v.iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
                let kbps = values(|p| p.video.bitrate_kbps);
                let mut kinds: Vec<&str> = scope.iter().map(|p| p.video.profile.onvif()).collect();
                kinds.dedup();
                reply(format!(
                    "<tr2:Options GovLengthRange=\"{}\" FrameRatesSupported=\"{}\" ProfilesSupported=\"{}\" ConstantBitRateSupported=\"false\">{}{}{resolutions}{}</tr2:Options>",
                    list(values(|p| p.video.gop)),
                    list(values(|p| p.video.fps)),
                    kinds.join(" "),
                    text("tt:Encoding", "H264"),
                    range("tt:QualityRange", 0, 10),
                    range(
                        "tt:BitrateRange",
                        kbps.first().copied().unwrap_or(0),
                        kbps.last().copied().unwrap_or(0)
                    ),
                ))
            }
            Err(fault) => fault,
        },
        "GetAudioSourceConfigurations" => single(ctx, req, AUDIO_SOURCE_CONFIG, || {
            audio_source_config("tr2:Configurations", uses)
        }),
        "GetAudioSourceConfigurationOptions" => reply(el(
            "tr2:Options",
            text("tt:InputTokensAvailable", super::AUDIO_SOURCE),
        )),
        "GetAudioEncoderConfigurations" => single(ctx, req, AUDIO_ENCODER, || {
            audio_encoder("tr2:Configurations", uses)
        }),
        "GetAudioEncoderConfigurationOptions" => reply(audio_options()),
        "GetAudioOutputConfigurations" if ctx.camera.audio_output() => {
            single(ctx, req, AUDIO_OUTPUT_CONFIG, || {
                audio_output_config("tr2:Configurations", uses)
            })
        }
        "GetAudioOutputConfigurationOptions" if ctx.camera.audio_output() => reply(el(
            "tr2:Options",
            format!(
                "{}<tt:SendPrimacyOptions>www.onvif.org/ver20/HalfDuplex/Auto</tt:SendPrimacyOptions>{}",
                text("tt:OutputTokensAvailable", AUDIO_OUTPUT),
                range("tt:OutputLevelRange", 100, 100)
            ),
        )),
        "GetAudioDecoderConfigurations" if ctx.camera.audio_output() => {
            single(ctx, req, AUDIO_DECODER, || {
                audio_decoder_config("tr2:Configurations", uses)
            })
        }
        "GetAudioDecoderConfigurationOptions" if ctx.camera.audio_output() => {
            reply(audio_options())
        }
        // No metadata stream and no analytics engine: nothing to list.
        "GetMetadataConfigurations" | "GetAnalyticsConfigurations" => reply(String::new()),
        "GetVideoEncoderInstances" => {
            if req.body.text_of("ConfigurationToken") != Some(VIDEO_SOURCE_CONFIG) {
                return no_config();
            }
            reply(format!(
                "<tr2:Info><tr2:Codec>{}{}</tr2:Codec>{}</tr2:Info>",
                text("tr2:Encoding", "H264"),
                text("tr2:Number", uses),
                text("tr2:Total", uses)
            ))
        }
        "GetStreamUri" => {
            // `RTSP` is "over UDP or TCP interleaved", which is what the server speaks; multicast and
            // RTSP over HTTP/TLS are not offered.
            if !matches!(req.body.text_of("Protocol"), Some("RtspUnicast" | "RTSP")) {
                return Reply::Fault(Fault::invalid_arg(
                    "ter:InvalidStreamSetup",
                    "Only RtspUnicast and RTSP streams are offered",
                ));
            }
            match find_profile(ctx, req) {
                Some(p) => reply(text(
                    "tr2:Uri",
                    format!("rtsp://{}:{}/{}", ctx.host, ctx.rtsp_port, p.stream_path),
                )),
                None => no_profile(),
            }
        }
        "GetSnapshotUri" => match find_profile(ctx, req) {
            Some(_) => reply(text("tr2:Uri", url(ctx, "/snapshot.jpg"))),
            None => no_profile(),
        },
        // See Media1: key frames come every second, so nothing needs forcing.
        "SetSynchronizationPoint" => match find_profile(ctx, req) {
            Some(_) => reply(String::new()),
            None => no_profile(),
        },
        "SetVideoEncoderConfiguration" => {
            let Some(config) = req.body.child("Configuration") else {
                return no_config();
            };
            let Some(p) = config.attr("token").and_then(|t| encoder_for(ctx, t)) else {
                return no_config();
            };
            match same_encoder(config, &p) {
                Some(true) => reply(String::new()),
                _ => read_only("Encoder settings are read-only over ONVIF"),
            }
        }
        "SetAudioEncoderConfiguration" => {
            let Some(config) = req.body.child("Configuration") else {
                return no_config();
            };
            let same = config.attr("token") == Some(AUDIO_ENCODER)
                && config.text_of("Encoding").is_none_or(|e| e == "PCMU")
                && config.text_of("SampleRate").is_none_or(|r| r == "8");
            if same {
                reply(String::new())
            } else {
                read_only("Audio is G.711 µ-law (PCMU) at 8 kHz")
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
                read_only("Configurations are read-only over ONVIF")
            }
        }
        // A fixed profile already holds every configuration: re-adding its own is a no-op, anything else
        // cannot be added, and nothing can be removed.
        "AddConfiguration" => {
            let Some(p) = find_profile(ctx, req) else {
                return no_profile();
            };
            let held = held(ctx, &p);
            let all_held = req.body.children_named("Configuration").all(|c| {
                c.text_of("Token")
                    .is_some_and(|t| held.iter().any(|h| h == t))
            });
            if all_held {
                reply(String::new())
            } else {
                no_config()
            }
        }
        "RemoveConfiguration" => match find_profile(ctx, req) {
            Some(_) => read_only("Profiles are fixed"),
            None => no_profile(),
        },
        _ => Reply::Fault(Fault::action_not_supported()),
    }
}

fn read_only(why: &str) -> Reply {
    Reply::Fault(Fault::invalid_arg("ter:ConfigModify", why.to_string()))
}

/// `AudioEncoder2ConfigurationOptions`: the one codec this camera speaks.
fn audio_options() -> String {
    "<tr2:Options><tt:Encoding>PCMU</tt:Encoding><tt:BitrateList><tt:Items>64</tt:Items></tt:BitrateList><tt:SampleRateList><tt:Items>8</tt:Items></tt:SampleRateList></tr2:Options>".into()
}

/// The configuration tokens a profile already holds.
fn held(ctx: &Context, p: &MediaProfile) -> Vec<String> {
    let mut held = vec![VIDEO_SOURCE_CONFIG.to_string(), video_encoder_token(p)];
    if p.audio {
        held.extend([AUDIO_SOURCE_CONFIG.into(), AUDIO_ENCODER.into()]);
    }
    if ctx.camera.ptz().is_some() {
        held.push(PTZ_CONFIG.into());
    }
    if ctx.camera.audio_output() {
        held.extend([AUDIO_OUTPUT_CONFIG.into(), AUDIO_DECODER.into()]);
    }
    held
}
