//! The `cam` subcommands: one table says what each is called, what it owns and how it runs.

use std::fmt::Display;
use std::path::Path;
use std::process::ExitCode;

use crate::ownership::{self, Resource};

mod args;
pub mod dump;
pub mod health;
pub mod ir;
pub mod net;
pub mod ptz;
pub mod serve;
pub mod wdt;
pub mod zoom;

#[derive(Debug, PartialEq, Eq)]
pub enum CliError {
    /// Bad arguments: the command's usage is printed and it exits 2.
    Usage,
    /// The command ran and failed: `<name> failed: <reason>`, exit 1.
    Failed(String),
}

impl<E: std::error::Error> From<E> for CliError {
    fn from(e: E) -> Self {
        Self::Failed(e.to_string())
    }
}

pub fn failed(e: impl Display) -> CliError {
    CliError::Failed(e.to_string())
}

/// What a command holds while it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ownership {
    Nothing,
    /// The shared runtime lock, plus one resource only this diagnostic may hold.
    Diagnostic(Option<Resource>),
    /// The command takes the runtime lock exclusively itself.
    Production,
}

#[derive(Clone, Copy)]
pub enum Owns {
    Never,
    Always(Ownership),
    /// Decided from the command's arguments.
    Depends(fn(&[String]) -> Ownership),
}

type Run = fn(&Path, &[String]) -> Result<ExitCode, CliError>;
/// Handles the command without any lease when it can, as a PTZ stop sent to a running owner.
type Delegate = fn(&Path, &[String]) -> Option<Result<ExitCode, CliError>>;

pub struct Command {
    pub name: &'static str,
    synopsis: &'static str,
    usage: &'static str,
    takes_args: bool,
    pub owns: Owns,
    delegate: Option<Delegate>,
    run: Run,
}

const CONTROLS: Owns = Owns::Always(Ownership::Diagnostic(Some(Resource::Controls)));

pub const COMMANDS: &[Command] = &[
    Command {
        name: "self-test",
        synopsis: "self-test",
        usage: "usage: cam self-test",
        takes_args: false,
        owns: Owns::Never,
        delegate: None,
        run: |_, _| self_test(),
    },
    Command {
        name: "probe-videocap",
        synopsis: "probe-videocap",
        usage: "usage: cam probe-videocap",
        takes_args: false,
        owns: Owns::Always(Ownership::Diagnostic(Some(Resource::Media))),
        delegate: None,
        run: |_, _| dump::probe(),
    },
    Command {
        name: "dump",
        synopsis: "dump",
        usage: dump::USAGE,
        takes_args: true,
        owns: Owns::Always(Ownership::Diagnostic(Some(Resource::Media))),
        delegate: None,
        run: |_, args| dump::run(args),
    },
    Command {
        name: "serve",
        synopsis: "serve",
        usage: serve::USAGE,
        takes_args: true,
        owns: Owns::Depends(serve::owns),
        delegate: None,
        run: |_, args| serve::run(args),
    },
    Command {
        name: "preflight",
        synopsis: "preflight",
        usage: "usage: cam preflight",
        takes_args: false,
        owns: Owns::Never,
        delegate: None,
        run: |_, _| Ok(health::preflight()),
    },
    Command {
        name: "health-watch",
        synopsis: "health-watch",
        usage: health::USAGE,
        takes_args: true,
        owns: Owns::Never,
        delegate: None,
        run: |_, args| health::watch(args),
    },
    Command {
        name: "ptz",
        synopsis: "ptz ...",
        usage: ptz::USAGE,
        takes_args: true,
        owns: CONTROLS,
        delegate: Some(ptz::delegate_stop),
        run: ptz::run,
    },
    Command {
        name: "zoom",
        synopsis: "zoom ...",
        usage: zoom::USAGE,
        takes_args: true,
        owns: CONTROLS,
        delegate: None,
        run: |_, args| zoom::run_zoom(args),
    },
    Command {
        name: "focus",
        synopsis: "focus ...",
        usage: zoom::USAGE,
        takes_args: true,
        owns: CONTROLS,
        delegate: None,
        run: |_, args| zoom::run_focus(args),
    },
    Command {
        name: "ir",
        synopsis: "ir ...",
        usage: ir::USAGE,
        takes_args: true,
        owns: CONTROLS,
        delegate: None,
        run: |_, args| ir::run_ir(args),
    },
    Command {
        name: "led",
        synopsis: "led ...",
        usage: ir::USAGE,
        takes_args: true,
        owns: CONTROLS,
        delegate: None,
        run: |_, args| ir::run_led(args),
    },
    Command {
        name: "wdt",
        synopsis: "wdt",
        usage: "usage: cam wdt",
        takes_args: false,
        owns: Owns::Always(Ownership::Diagnostic(None)),
        delegate: None,
        run: |_, _| Ok(wdt::run()),
    },
    Command {
        name: "net",
        synopsis: "net",
        usage: net::USAGE,
        takes_args: true,
        owns: Owns::Depends(net::owns),
        delegate: None,
        run: |_, args| net::run(args),
    },
];

/// Whether a `cam <subcommand>` process may hold the watchdog, HDAL, the network or the controls.
pub fn owns_hardware(subcommand: &str) -> bool {
    COMMANDS
        .iter()
        .any(|c| c.name == subcommand && !matches!(c.owns, Owns::Never))
}

/// Runs `cam <args>`. `runtime` is the runtime lock; the other lock files and the stop endpoint sit beside
/// it, so tests root them all in a fixture. A lease that cannot be taken is the error.
pub fn dispatch(args: &[String], runtime: &Path) -> std::io::Result<ExitCode> {
    let Some(command) = args
        .first()
        .and_then(|name| COMMANDS.iter().find(|c| c.name == name))
    else {
        let names: Vec<_> = COMMANDS.iter().map(|c| c.synopsis).collect();
        eprintln!("usage: cam {}", names.join(" | "));
        return Ok(ExitCode::from(2));
    };
    let rest = &args[1..];
    if !command.takes_args && !rest.is_empty() {
        return Ok(command.finish(Err(CliError::Usage)));
    }
    if let Some(result) = command
        .delegate
        .and_then(|delegate| delegate(runtime, rest))
    {
        return Ok(command.finish(result));
    }
    let ownership = match command.owns {
        Owns::Never => Ownership::Nothing,
        Owns::Always(ownership) => ownership,
        Owns::Depends(decide) => decide(rest),
    };
    // Held until the command and all its workers return; `net` exits the process with them still held.
    let _leases = match ownership {
        Ownership::Diagnostic(resource) => {
            let shared = ownership::shared_diagnostic(runtime)?;
            let exclusive = resource
                .map(|r| ownership::exclusive(runtime, r))
                .transpose()?;
            // Tuple fields drop in order: the resource is released before the shared lock.
            Some((exclusive, shared))
        }
        Ownership::Nothing | Ownership::Production => None,
    };
    Ok(command.finish((command.run)(runtime, rest)))
}

impl Command {
    fn finish(&self, result: Result<ExitCode, CliError>) -> ExitCode {
        match result {
            Ok(code) => code,
            Err(CliError::Usage) => {
                eprintln!("{}", self.usage);
                ExitCode::from(2)
            }
            Err(CliError::Failed(reason)) => {
                eprintln!("{} failed: {reason}", self.name);
                ExitCode::FAILURE
            }
        }
    }
}

fn self_test() -> Result<ExitCode, CliError> {
    use crate::selftest::{self, Libhdal};
    let hdal = Libhdal::open("/usr/lib/libhdal.so")?;
    print!("{}", selftest::report(&hdal)?);
    Ok(ExitCode::SUCCESS)
}

/// A command body generic over the HDAL backend, so it builds and is tested on every target.
pub trait OnHdal {
    fn run<B>(self, hdal: B) -> Result<ExitCode, CliError>
    where
        B: hdal::Backend + hdal::enc::EncBackend + hdal::AudioCap + hdal::AudioOut;
}

#[cfg(target_arch = "arm")]
fn on_hdal(body: impl OnHdal) -> Result<ExitCode, CliError> {
    body.run(camera_hdal())
}

/// The camera's `libhdal.so`, with the speaker amplifier switched by its GPIO and the codec's record gain
/// set through `/dev/mem` (best effort).
#[cfg(target_arch = "arm")]
pub fn camera_hdal() -> hdal::sys::Sys {
    use crate::ir::sysfs::{GPIO_ROOT, Gpio, SPEAKER_ENABLE_GPIO};
    let gpio = Gpio::new(GPIO_ROOT);
    hdal::sys::Sys::new()
        .with_speaker_enable(move |on| {
            gpio.set(SPEAKER_ENABLE_GPIO, on).map_err(|error| {
                eprintln!("speaker amplifier: {error}");
                -5
            })
        })
        // Without `/dev/mem` the microphone keeps the codec's stock ALC and noise gate rather than going down.
        .with_mic_gain(|volume| {
            if let Err(error) = crate::eac::set_record_gain(volume) {
                eprintln!("mic gain: {error}; codec left as stock");
            }
            Ok(())
        })
}

#[cfg(not(target_arch = "arm"))]
fn on_hdal(_body: impl OnHdal) -> Result<ExitCode, CliError> {
    Err(failed("needs the camera's libhdal.so (armv7 build)"))
}
