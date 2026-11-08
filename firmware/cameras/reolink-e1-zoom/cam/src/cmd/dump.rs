//! `cam probe-videocap` and `cam dump`: HDAL experiments, only with the vendor stack killed, the watchdog held
//! by `cam wdt` and a reboot afterwards.

use std::fs::File;
use std::io::BufWriter;
use std::process::ExitCode;

use super::{CliError, OnHdal, failed, on_hdal};
use crate::annexb::AnnexB;
use crate::pipeline::{self, PipelineConfig};

pub const USAGE: &str = "usage: cam dump [--seconds N] [--out DIR]";

/// HDAL init, memory pool, capture sysinfo, clean up.
pub fn probe() -> Result<ExitCode, CliError> {
    struct Probe;
    impl OnHdal for Probe {
        fn run<B: hdal::Backend>(self, mut hdal: B) -> Result<ExitCode, CliError> {
            probe_with(&mut hdal)
        }
    }
    on_hdal(Probe)
}

pub fn probe_with(hw: &mut impl hdal::Backend) -> Result<ExitCode, CliError> {
    let out = hdal::probe_videocap(hw, &hdal::ProbeConfig::os05a10())?;
    print!("{}", out.report());
    Ok(if out.cleanup_errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    pub seconds: u32,
    pub dir: String,
}

pub fn parse(args: &[String]) -> Result<Args, CliError> {
    let mut parsed = Args {
        seconds: 10,
        dir: "/mnt/tmp".into(),
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match (a.as_str(), it.next()) {
            ("--seconds", Some(v)) => parsed.seconds = v.parse().map_err(|_| CliError::Usage)?,
            ("--out", Some(v)) => parsed.dir = v.clone(),
            _ => return Err(CliError::Usage),
        }
    }
    Ok(parsed)
}

/// Runs the capture → videoproc → two-encoder graph and writes `main.h264` and `sub.h264` (Annex B) to the
/// output directory.
pub fn run(args: &[String]) -> Result<ExitCode, CliError> {
    on_hdal(parse(args)?)
}

impl OnHdal for Args {
    fn run<B: hdal::Backend + hdal::enc::EncBackend>(
        self,
        mut hdal: B,
    ) -> Result<ExitCode, CliError> {
        let dir = &self.dir;
        let open = |name: &str| File::create(format!("{dir}/{name}")).map(BufWriter::new);
        let (main_f, sub_f) = match (open("main.h264"), open("sub.h264")) {
            (Ok(m), Ok(s)) => (m, s),
            (Err(e), _) | (_, Err(e)) => {
                eprintln!("dump: cannot create output in {dir}: {e}");
                return Ok(ExitCode::FAILURE);
            }
        };
        let mut sinks = [AnnexB::new(main_f), AnnexB::new(sub_f)];
        let mut cpu =
            camera_transport::telemetry::Sampler::new(camera_transport::telemetry::Proc::default());
        cpu.sample();
        let report = pipeline::dump(
            &mut hdal,
            &PipelineConfig::e1_zoom(),
            self.seconds,
            &mut sinks,
        )
        .map_err(failed)?;
        let busy = cpu.sample().cpu_busy;
        for (name, s) in ["main", "sub"].iter().zip(report.streams) {
            println!(
                "{name}: frames={} key_frames={} bytes={}",
                s.frames, s.key_frames, s.bytes
            );
        }
        println!("elapsed_ms={}", report.elapsed_ms);
        if let Some(busy) = busy {
            println!("cpu_busy_percent={busy:.1}");
        }
        Ok(ExitCode::SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::args::words as args;

    #[test]
    fn seconds_and_output_directory_are_optional() {
        assert_eq!(
            parse(&args("")),
            Ok(Args {
                seconds: 10,
                dir: "/mnt/tmp".into()
            })
        );
        assert_eq!(
            parse(&args("--out /x --seconds 3")),
            Ok(Args {
                seconds: 3,
                dir: "/x".into()
            })
        );
        for bad in ["--seconds", "--seconds x", "--seconds -1", "--bogus 1", "x"] {
            assert_eq!(parse(&args(bad)), Err(CliError::Usage), "{bad:?}");
        }
    }
}
