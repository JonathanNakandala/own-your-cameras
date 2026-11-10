//! The Imaging service: the picture's brightness, colour saturation, contrast and sharpness, the IR-cut
//! filter (day/night) and the focus mode. The camera maps each onto its own settings
//! ([`crate::ImagingPort`]); what it does not have is left out of the replies and ignored in requests, so a
//! client that sends back the whole settings document it read, with sections of its own, still works.
//! Focus *moves* (`Move`, `GetMoveOptions`, `GetStatus`) are not offered.

use super::{Context, Reply, VIDEO_SOURCE, el, range, response, text};
use crate::camera::{ImagingError, ImagingPort, ImagingSettings, IrCutFilter};
use crate::soap::{Fault, Request};
use crate::xml::Element;

pub(super) fn capabilities() -> String {
    "<timg:Capabilities ImageStabilization=\"false\" Presets=\"false\" AdaptablePreset=\"false\"/>"
        .into()
}

fn ir_cut(mode: IrCutFilter) -> &'static str {
    match mode {
        IrCutFilter::On => "ON",
        IrCutFilter::Off => "OFF",
        IrCutFilter::Auto => "AUTO",
    }
}

/// `ImagingSettings20`, in the schema's order (brightness, saturation, contrast, focus, IR cut, sharpness).
fn settings_xml(s: &ImagingSettings) -> String {
    let level = |name: &str, v: Option<f32>| v.map(|v| text(name, v)).unwrap_or_default();
    let focus = s
        .auto_focus
        .map(|auto| {
            el(
                "tt:Focus",
                text("tt:AutoFocusMode", if auto { "AUTO" } else { "MANUAL" }),
            )
        })
        .unwrap_or_default();
    format!(
        "{}{}{}{focus}{}{}",
        level("tt:Brightness", s.brightness),
        level("tt:ColorSaturation", s.saturation),
        level("tt:Contrast", s.contrast),
        s.ir_cut
            .map(|m| text("tt:IrCutFilter", ir_cut(m)))
            .unwrap_or_default(),
        level("tt:Sharpness", s.sharpness),
    )
}

fn options_xml(port: &dyn ImagingPort) -> String {
    let o = port.options();
    let level = |name: &str, r: Option<(f32, f32)>| {
        r.map(|(min, max)| range(name, min, max))
            .unwrap_or_default()
    };
    let focus = if o.auto_focus {
        el(
            "tt:Focus",
            "<tt:AutoFocusModes>AUTO</tt:AutoFocusModes><tt:AutoFocusModes>MANUAL</tt:AutoFocusModes>",
        )
    } else {
        String::new()
    };
    let modes = if o.ir_cut {
        "<tt:IrCutFilterModes>ON</tt:IrCutFilterModes><tt:IrCutFilterModes>OFF</tt:IrCutFilterModes><tt:IrCutFilterModes>AUTO</tt:IrCutFilterModes>"
    } else {
        ""
    };
    format!(
        "{}{}{}{focus}{modes}{}",
        level("tt:Brightness", o.brightness),
        level("tt:ColorSaturation", o.saturation),
        level("tt:Contrast", o.contrast),
        level("tt:Sharpness", o.sharpness),
    )
}

fn invalid(why: &str) -> Fault {
    Fault::invalid_arg("ter:SettingsInvalid", why.to_string())
}

fn level(e: &Element, name: &str) -> Result<Option<f32>, Fault> {
    let Some(c) = e.child(name) else {
        return Ok(None);
    };
    c.text
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .map(Some)
        .ok_or_else(|| invalid(&format!("{name} is not a number")))
}

/// The settings a `SetImagingSettings` body asks for; sections the camera has no use for are ignored.
fn requested(body: &Element) -> Result<ImagingSettings, Fault> {
    let Some(e) = body.child("ImagingSettings") else {
        return Err(invalid("No ImagingSettings"));
    };
    let ir_cut = match e.text_of("IrCutFilter") {
        None => None,
        Some("ON") => Some(IrCutFilter::On),
        Some("OFF") => Some(IrCutFilter::Off),
        Some("AUTO") => Some(IrCutFilter::Auto),
        Some(_) => return Err(invalid("IrCutFilter is ON, OFF or AUTO")),
    };
    let auto_focus = match e.child("Focus").and_then(|f| f.text_of("AutoFocusMode")) {
        None => None,
        Some("AUTO") => Some(true),
        Some("MANUAL") => Some(false),
        Some(_) => return Err(invalid("AutoFocusMode is AUTO or MANUAL")),
    };
    Ok(ImagingSettings {
        brightness: level(e, "Brightness")?,
        saturation: level(e, "ColorSaturation")?,
        contrast: level(e, "Contrast")?,
        sharpness: level(e, "Sharpness")?,
        ir_cut,
        auto_focus,
    })
}

fn failed(e: ImagingError) -> Fault {
    match e {
        ImagingError::Invalid(why) => invalid(&why),
        ImagingError::Failed(why) => Fault::receiver(&["ter:Action"], why),
    }
}

pub(super) fn handle(req: &Request, _ctx: &Context, port: &dyn ImagingPort) -> Reply {
    let action = req.action.as_str();
    let reply = |inner: String| response(action, "timg", inner);
    if action == "GetServiceCapabilities" {
        return reply(capabilities());
    }
    if !matches!(
        action,
        "GetImagingSettings" | "SetImagingSettings" | "GetOptions"
    ) {
        return Reply::Fault(Fault::action_not_supported());
    }
    if req.body.text_of("VideoSourceToken") != Some(VIDEO_SOURCE) {
        return Reply::Fault(Fault::invalid_arg("ter:NoSource", "No such video source"));
    }
    match action {
        "GetImagingSettings" => reply(el("timg:ImagingSettings", settings_xml(&port.settings()))),
        "GetOptions" => reply(el("timg:ImagingOptions", options_xml(port))),
        _ => match requested(&req.body).and_then(|s| port.apply(s).map_err(failed)) {
            Ok(()) => reply(String::new()),
            Err(f) => Reply::Fault(f),
        },
    }
}
