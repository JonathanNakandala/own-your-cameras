//! The trial boot of the 3D noise reduction graph.
//!
//! A graph that the video processor does not accept stalls every output, a stalled stream resets the camera
//! through the watchdog, and three counted starts select the vendor stack (`docs/recovery.md`).
//! So a start that turns the feature on first writes a marker; a start that finds the marker knows the last
//! one never proved healthy and runs the plain graph instead. The media loop deletes the marker once both
//! streams delivered for [`crate::pipeline::TRIAL_PROVEN_MS`].
//!
//! | setting `isp_3dnr` | main stream | marker | this start | marker after |
//! |---|---|---|---|---|
//! | off | any | present | plain graph | removed (the way to retry: off, reboot, on, reboot) |
//! | off | any | absent | plain graph | absent |
//! | on | not the full sensor frame | any | plain graph | untouched |
//! | on | full sensor frame | present | plain graph, logged | present |
//! | on | full sensor frame | absent | 3D noise reduction | written first, removed when proven |

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Where production keeps the marker (a RAM-only diagnostic run uses `/mnt/tmp`).
pub const MARKER: &str = "/mnt/para/cam-3dnr.trial";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trial {
    path: PathBuf,
}

/// What this start does about 3D noise reduction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Build the plain graph.
    Plain,
    /// Build the graph with 3D noise reduction; the marker is written.
    Trial,
}

impl Trial {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Decides, with the file effects of the table above. A marker that cannot be written means no trial:
    /// the feature fails closed.
    pub fn decide(&self, wanted: bool, full_size_main: bool) -> Decision {
        let present = self.path.exists();
        if !wanted {
            if present {
                match fs::remove_file(&self.path) {
                    Ok(()) => eprintln!(
                        "denoise: 3D noise reduction is off; the failed-trial marker is cleared"
                    ),
                    Err(e) => eprintln!("denoise: cannot clear {}: {e}", self.path.display()),
                }
            }
            return Decision::Plain;
        }
        if !full_size_main {
            eprintln!(
                "denoise: 3D noise reduction needs the 2592x1944 main stream; running without it"
            );
            return Decision::Plain;
        }
        if present {
            eprintln!(
                "denoise: the last 3D noise reduction start never proved healthy; running without it \
                 (turn isp_3dnr off, reboot, turn it on and reboot to try again)"
            );
            return Decision::Plain;
        }
        match self.write() {
            Ok(()) => Decision::Trial,
            Err(e) => {
                eprintln!(
                    "denoise: cannot write {}: {e}; running without 3D noise reduction",
                    self.path.display()
                );
                Decision::Plain
            }
        }
    }

    /// The trial is proven: both streams delivered long enough.
    pub fn finish(&self) {
        match fs::remove_file(&self.path) {
            Ok(()) => eprintln!("denoise: 3D noise reduction proven healthy"),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => eprintln!("denoise: cannot clear {}: {e}", self.path.display()),
        }
    }

    /// Created exclusively and synced with its directory, so a reset right after cannot lose it.
    fn write(&self) -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.path)?;
        file.write_all(b"trial\n")?;
        file.sync_all()?;
        match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => fs::File::open(dir)?.sync_all(),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> Trial {
        let d = std::env::temp_dir().join(format!("cam-trial-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        Trial::new(d.join("cam-3dnr.trial"))
    }

    #[test]
    fn a_first_start_with_the_setting_on_writes_the_marker_and_runs_the_trial() {
        let t = scratch("first");
        assert_eq!(t.decide(true, true), Decision::Trial);
        assert!(t.path().exists());
    }

    #[test]
    fn the_next_start_finds_the_marker_and_runs_plain() {
        let t = scratch("next");
        assert_eq!(t.decide(true, true), Decision::Trial);
        assert_eq!(t.decide(true, true), Decision::Plain);
        assert!(
            t.path().exists(),
            "the marker stays until a human clears it"
        );
    }

    #[test]
    fn a_proven_trial_clears_the_marker_so_the_next_start_runs_it_again() {
        let t = scratch("proven");
        assert_eq!(t.decide(true, true), Decision::Trial);
        t.finish();
        assert!(!t.path().exists());
        assert_eq!(t.decide(true, true), Decision::Trial);
    }

    #[test]
    fn turning_the_setting_off_clears_a_failed_trial() {
        let t = scratch("off");
        assert_eq!(t.decide(true, true), Decision::Trial);
        assert_eq!(t.decide(false, true), Decision::Plain);
        assert!(!t.path().exists());
        assert_eq!(t.decide(true, true), Decision::Trial);
    }

    #[test]
    fn the_setting_off_touches_nothing_when_there_is_no_marker() {
        let t = scratch("quiet");
        assert_eq!(t.decide(false, true), Decision::Plain);
        assert!(!t.path().exists());
    }

    #[test]
    fn a_main_stream_that_is_not_full_size_runs_plain_and_leaves_the_marker_alone() {
        let t = scratch("small");
        assert_eq!(t.decide(true, false), Decision::Plain);
        assert!(!t.path().exists());
        assert_eq!(t.decide(true, true), Decision::Trial);
        assert_eq!(t.decide(true, false), Decision::Plain);
        assert!(t.path().exists());
    }

    #[test]
    fn a_marker_that_cannot_be_written_means_no_trial() {
        let t = Trial::new("/nonexistent-dir-for-cam-trial/cam-3dnr.trial");
        assert_eq!(t.decide(true, true), Decision::Plain);
    }

    #[test]
    fn finishing_without_a_marker_is_quiet() {
        scratch("nothing").finish();
    }
}
