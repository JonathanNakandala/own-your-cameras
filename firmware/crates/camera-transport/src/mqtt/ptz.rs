//! The shared pan/tilt/zoom command vocabulary: axis numbers, relative moves, held moves, the
//! `motion` JSON contract and `position3d` taps. Cameras choose the syntax they publish.
use super::Command;
use crate::{motion_control, position3d};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Position {
    Absolute(i32),
    Relative(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
    UpLeft,
    UpRight,
    DownLeft,
    DownRight,
}

impl Direction {
    /// Screen coordinates: positive pan is right, positive tilt is up.
    pub const fn axes(self) -> (i32, i32) {
        match self {
            Self::Left => (-1, 0),
            Self::Right => (1, 0),
            Self::Up => (0, 1),
            Self::Down => (0, -1),
            Self::UpLeft => (-1, 1),
            Self::UpRight => (1, 1),
            Self::DownLeft => (-1, -1),
            Self::DownRight => (1, -1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zoom {
    In,
    Out,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ptz {
    Pan(Position),
    Tilt(Position),
    Zoom(Position),
    /// Held motion until Release.
    Move(Direction),
    ZoomMove(Zoom),
    Velocity(motion_control::Velocity),
    Home,
    Preset(motion_control::PresetName),
    SavePreset(motion_control::PresetName),
    RemovePreset(motion_control::PresetName),
    Position3d(position3d::Request),
}

/// How relative axis moves are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relative {
    /// `<axis>_relative/set` with a step count up to `limit` either way.
    Topic { limit: i32 },
    /// `relative:N` on the axis topic, up to the axis range either way.
    Prefix,
}

/// Which PTZ commands a camera accepts and how it writes them. An axis maximum of `None` means the
/// camera has no such axis; its commands are not recognised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Syntax {
    pub pan: Option<i32>,
    pub tilt: Option<i32>,
    pub zoom: Option<i32>,
    pub relative: Relative,
    /// Accept an integral `N.0` on axis topics (Home Assistant sliders send it).
    pub fractions: bool,
    /// `move/set` also accepts `{"pan":-1..1,"tilt":-1..1}`.
    pub vectors: bool,
    /// What `move/set stop` means: Release (graceful) when true, Stop when false.
    pub move_stop_releases: bool,
    pub position3d: bool,
}

/// Parses `id`/`text` as a PTZ command. `None`: not a PTZ command of this syntax.
pub fn parse(syntax: &Syntax, id: &str, text: &str) -> Option<Result<Command<Ptz>, String>> {
    let axis = |name: &str| match name {
        "pan" => syntax.pan.map(|max| (max, Ptz::Pan as fn(Position) -> Ptz)),
        "tilt" => syntax
            .tilt
            .map(|max| (max, Ptz::Tilt as fn(Position) -> Ptz)),
        "zoom" => syntax
            .zoom
            .map(|max| (max, Ptz::Zoom as fn(Position) -> Ptz)),
        _ => None,
    };
    let scalar = text.trim();
    if let Some((max, make)) = axis(id) {
        if let (Relative::Prefix, Some(steps)) = (syntax.relative, scalar.strip_prefix("relative:"))
        {
            return Some(
                steps
                    .parse::<i32>()
                    .ok()
                    .filter(|n| (-max..=max).contains(n))
                    .map(|n| Command::App(make(Position::Relative(n))))
                    .ok_or_else(|| "relative position outside travel".into()),
            );
        }
        let number = if syntax.fractions {
            scalar.strip_suffix(".0").unwrap_or(scalar)
        } else {
            scalar
        };
        return Some(
            number
                .parse::<i32>()
                .ok()
                .filter(|n| (0..=max).contains(n))
                .map(|n| Command::App(make(Position::Absolute(n))))
                .ok_or_else(|| "position outside travel".into()),
        );
    }
    if let (Relative::Topic { limit }, Some(name)) = (syntax.relative, id.strip_suffix("_relative"))
        && let Some((_, make)) = axis(name)
    {
        return Some(
            scalar
                .parse::<i32>()
                .ok()
                .filter(|n| n.unsigned_abs() <= limit.unsigned_abs())
                .map(|n| Command::App(make(Position::Relative(n))))
                .ok_or_else(|| "relative position outside travel".into()),
        );
    }
    let moves = syntax.pan.is_some() || syntax.tilt.is_some();
    match id {
        "move" if moves => Some(held(syntax, scalar).ok_or_else(|| "invalid move".into())),
        "motion" if moves || syntax.zoom.is_some() => {
            Some(motion_control::Request::parse(text).map(motion))
        }
        "position3d" if syntax.position3d => {
            Some(position3d::Request::parse(text).map(|r| Command::App(Ptz::Position3d(r))))
        }
        "stop" if moves => Some(if scalar == "PRESS" {
            Ok(Command::Stop)
        } else {
            Err("invalid command payload".into())
        }),
        _ => None,
    }
}

fn held(syntax: &Syntax, text: &str) -> Option<Command<Ptz>> {
    let app = |p| Some(Command::App(p));
    match text {
        "left" => app(Ptz::Move(Direction::Left)),
        "right" => app(Ptz::Move(Direction::Right)),
        "up" => app(Ptz::Move(Direction::Up)),
        "down" => app(Ptz::Move(Direction::Down)),
        "zoom_in" if syntax.zoom.is_some() => app(Ptz::ZoomMove(Zoom::In)),
        "zoom_out" if syntax.zoom.is_some() => app(Ptz::ZoomMove(Zoom::Out)),
        "release" => Some(Command::Release),
        "stop" if syntax.move_stop_releases => Some(Command::Release),
        "stop" => Some(Command::Stop),
        _ if syntax.vectors => vector(text),
        _ => None,
    }
}

fn vector(text: &str) -> Option<Command<Ptz>> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Vector {
        pan: i8,
        tilt: i8,
    }
    let v: Vector = serde_json::from_str(text).ok()?;
    let direction = match (v.pan, v.tilt) {
        (0, 0) => return Some(Command::Release),
        (-1, 0) => Direction::Left,
        (1, 0) => Direction::Right,
        (0, 1) => Direction::Up,
        (0, -1) => Direction::Down,
        (-1, 1) => Direction::UpLeft,
        (1, 1) => Direction::UpRight,
        (-1, -1) => Direction::DownLeft,
        (1, -1) => Direction::DownRight,
        _ => return None,
    };
    Some(Command::App(Ptz::Move(direction)))
}

/// The `motion` JSON contract, one arm per request kind.
fn motion(request: motion_control::Request) -> Command<Ptz> {
    use motion_control::Request;
    let tap = |start, end, operation| {
        Command::App(Ptz::Position3d(position3d::Request {
            start,
            end,
            operation,
        }))
    };
    match request {
        Request::Velocity(v) => Command::App(Ptz::Velocity(v)),
        Request::Release => Command::Release,
        Request::Stop => Command::Stop,
        Request::Home => Command::App(Ptz::Home),
        Request::Preset(name) => Command::App(Ptz::Preset(name)),
        Request::SavePreset(name) => Command::App(Ptz::SavePreset(name)),
        Request::RemovePreset(name) => Command::App(Ptz::RemovePreset(name)),
        Request::Region {
            start,
            end,
            zoom_out,
        } => tap(
            start,
            end,
            if zoom_out {
                position3d::Operation::ZoomOut
            } else {
                position3d::Operation::ZoomTo
            },
        ),
        Request::Point { x, y } => tap((x, y), (x, y), position3d::Operation::ZoomTo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MI360: Syntax = Syntax {
        pan: Some(2404),
        tilt: Some(713),
        zoom: None,
        relative: Relative::Topic { limit: 256 },
        fractions: true,
        vectors: true,
        move_stop_releases: false,
        position3d: false,
    };
    const REOLINK: Syntax = Syntax {
        pan: Some(7360),
        tilt: Some(1240),
        zoom: Some(1024),
        relative: Relative::Prefix,
        fractions: false,
        vectors: false,
        move_stop_releases: true,
        position3d: true,
    };
    fn ok(s: &Syntax, id: &str, text: &str) -> Command<Ptz> {
        parse(s, id, text).unwrap().unwrap()
    }
    fn bad(s: &Syntax, id: &str, text: &str) -> bool {
        matches!(parse(s, id, text), Some(Err(_)) | None)
    }

    #[test]
    fn axes_take_absolute_positions_within_travel() {
        assert_eq!(
            ok(&MI360, "pan", "2404"),
            Command::App(Ptz::Pan(Position::Absolute(2404)))
        );
        assert_eq!(
            ok(&MI360, "tilt", "713.0"),
            Command::App(Ptz::Tilt(Position::Absolute(713)))
        );
        assert_eq!(
            ok(&REOLINK, "zoom", "1024"),
            Command::App(Ptz::Zoom(Position::Absolute(1024)))
        );
        for (s, id, text) in [
            (&MI360, "pan", "2405"),
            (&MI360, "tilt", "-1"),
            (&MI360, "tilt", "NaN"),
            (&REOLINK, "zoom", "1.5"),
            (&REOLINK, "tilt", "713.0"),
            (&REOLINK, "pan", "7361"),
        ] {
            assert!(bad(s, id, text), "{id} {text}");
        }
        assert!(parse(&MI360, "zoom", "1").is_none(), "no zoom axis");
    }

    #[test]
    fn relative_syntax_is_per_camera() {
        assert_eq!(
            ok(&MI360, "pan_relative", "-256"),
            Command::App(Ptz::Pan(Position::Relative(-256)))
        );
        assert!(bad(&MI360, "pan_relative", "257"));
        assert!(bad(&MI360, "pan", "relative:5"));
        assert_eq!(
            ok(&REOLINK, "pan", "relative:-7360"),
            Command::App(Ptz::Pan(Position::Relative(-7360)))
        );
        assert_eq!(
            ok(&REOLINK, "zoom", "relative:100"),
            Command::App(Ptz::Zoom(Position::Relative(100)))
        );
        assert!(bad(&REOLINK, "tilt", "relative:1241"));
        assert!(parse(&REOLINK, "pan_relative", "5").is_none());
    }

    #[test]
    fn held_moves_vectors_and_stop_meaning() {
        assert_eq!(
            ok(&MI360, "move", "left"),
            Command::App(Ptz::Move(Direction::Left))
        );
        assert_eq!(ok(&MI360, "move", "stop"), Command::Stop);
        assert_eq!(ok(&MI360, "move", "release"), Command::Release);
        for d in [Direction::UpLeft, Direction::DownRight, Direction::Up] {
            let (pan, tilt) = d.axes();
            assert_eq!(
                ok(&MI360, "move", &format!(r#"{{"pan":{pan},"tilt":{tilt}}}"#)),
                Command::App(Ptz::Move(d))
            );
        }
        assert_eq!(
            ok(&MI360, "move", r#"{"pan":0,"tilt":0}"#),
            Command::Release
        );
        for v in [
            r#"{"pan":2,"tilt":0}"#,
            r#"{"pan":1}"#,
            r#"{"pan":0,"tilt":1,"speed":9}"#,
            "null",
            "zoom_in",
        ] {
            assert!(bad(&MI360, "move", v), "{v}");
        }
        assert_eq!(ok(&REOLINK, "move", "stop"), Command::Release);
        assert_eq!(
            ok(&REOLINK, "move", "zoom_out"),
            Command::App(Ptz::ZoomMove(Zoom::Out))
        );
        assert!(bad(&REOLINK, "move", r#"{"pan":1,"tilt":0}"#));
        assert!(bad(&REOLINK, "move", "spin"));
        assert_eq!(ok(&REOLINK, "stop", "PRESS"), Command::Stop);
        assert!(bad(&REOLINK, "stop", "ON"));
    }

    #[test]
    fn motion_and_position3d_follow_the_shared_contracts() {
        assert_eq!(
            ok(&MI360, "motion", r#"{"op":"stop","v":1}"#),
            Command::Stop
        );
        assert_eq!(
            ok(&MI360, "motion", r#"{"op":"release","v":1}"#),
            Command::Release
        );
        assert_eq!(
            ok(&MI360, "motion", r#"{"op":"home","v":1}"#),
            Command::App(Ptz::Home)
        );
        assert!(bad(&MI360, "motion", "{}"));
        let point = r#"{"start_x":0,"start_y":255,"end_x":0,"end_y":255}"#;
        assert!(parse(&MI360, "position3d", point).is_none(), "disabled");
        assert!(
            matches!(ok(&REOLINK, "position3d", point), Command::App(Ptz::Position3d(r)) if r.is_point())
        );
        assert!(bad(&REOLINK, "position3d", "128,128"));
        assert!(parse(&MI360, "ir_mode", "day").is_none(), "not PTZ");
    }
}
