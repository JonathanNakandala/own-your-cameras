//! Compiles the pinned libopus release (`firmware/opus.json`, unpacked by `scripts/fetch-opus.py`) into a
//! static library with the pinned Zig, for the camera being built or for host tests.
use std::path::{Path, PathBuf};
use std::process::Command;

fn list(makefile: &Path, variable: &str) -> Vec<String> {
    let text = std::fs::read_to_string(makefile).expect("libopus source list");
    let mut lines = text
        .lines()
        .skip_while(|l| !l.starts_with(&format!("{variable} =")));
    let mut out = Vec::new();
    for line in lines.by_ref() {
        let continued = line.trim_end().ends_with('\\');
        let body = line
            .split_once('=')
            .map_or(line, |(k, v)| if k.trim() == variable { v } else { line });
        out.extend(
            body.trim_end_matches('\\')
                .split_whitespace()
                .map(str::to_owned),
        );
        if !continued {
            break;
        }
    }
    assert!(
        !out.is_empty(),
        "{variable} missing in {}",
        makefile.display()
    );
    out
}

fn main() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let workspace = manifest.join("../..");
    let lock = workspace.join("opus.json");
    println!("cargo:rerun-if-changed={}", lock.display());
    println!("cargo:rerun-if-changed=build.rs");
    let version = std::fs::read_to_string(&lock)
        .expect("opus.json")
        .split("\"version\": \"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .expect("opus.json version")
        .to_owned();
    let source = workspace.join(format!(".tools/opus-{version}"));
    let zig = workspace.join(".tools/zig/zig");
    assert!(
        source.join(".source-receipt.json").is_file(),
        "libopus source missing: run `python3 scripts/fetch-opus.py` in firmware/"
    );
    assert!(
        zig.is_file(),
        "Zig missing: run `python3 scripts/fetch-zig.py` in firmware/"
    );
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    let host = std::env::var("HOST").unwrap() == std::env::var("TARGET").unwrap();

    let mut sources = list(&source.join("opus_sources.mk"), "OPUS_SOURCES");
    sources.extend(list(&source.join("opus_sources.mk"), "OPUS_SOURCES_FLOAT"));
    sources.extend(list(&source.join("celt_sources.mk"), "CELT_SOURCES"));
    sources.extend(list(&source.join("silk_sources.mk"), "SILK_SOURCES"));
    sources.extend(list(&source.join("silk_sources.mk"), "SILK_SOURCES_FLOAT"));
    let mut flags: Vec<String> = ["-O2", "-ffunction-sections", "-fdata-sections", "-w"]
        .map(String::from)
        .to_vec();
    for define in ["OPUS_BUILD", "USE_ALLOCA", "HAVE_LRINTF", "HAVE_LRINT"] {
        flags.push(format!("-D{define}"));
    }
    for dir in ["", "include", "celt", "silk", "silk/float"] {
        flags.push("-I".into());
        flags.push(source.join(dir).display().to_string());
    }
    match arch.as_str() {
        // Reolink E1 Zoom: Cortex-A9 with NEON and no hardware divide (udiv-guard checks the result).
        "arm" if !host => {
            flags.extend(
                [
                    "-target",
                    "arm-linux-gnueabihf.2.30",
                    "-mcpu=cortex_a9",
                    "-mfpu=neon",
                ]
                .map(String::from),
            );
            flags.extend(
                ["OPUS_ARM_MAY_HAVE_NEON_INTR", "OPUS_ARM_PRESUME_NEON_INTR"]
                    .map(|d| format!("-D{d}")),
            );
            sources.extend(list(
                &source.join("celt_sources.mk"),
                "CELT_SOURCES_ARM_NEON_INTR",
            ));
            sources.extend(list(
                &source.join("silk_sources.mk"),
                "SILK_SOURCES_ARM_NEON_INTR",
            ));
        }
        // Mi 360: XBurst MIPS32r2 with an FPU, the same C flags as the IMP adapter objects. Optimised for size:
        // the application partition is full, and the -O2 build does not fit.
        "mips" if !host => {
            flags.retain(|f| f != "-O2");
            flags.push("-Os".into());
            flags.extend(
                [
                    "-target",
                    "mipsel-linux-gnueabihf",
                    "-mcpu=mips32r2",
                    "-fPIC",
                ]
                .map(String::from),
            );
        }
        _ if host => {}
        other => panic!("no libopus recipe for target arch {other}"),
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let objects = out.join("opus-obj");
    std::fs::create_dir_all(&objects).unwrap();
    let jobs = std::thread::available_parallelism().map_or(4, usize::from);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let built: Vec<PathBuf> = sources
        .iter()
        .map(|s| objects.join(s.replace('/', "_")).with_extension("o"))
        .collect();
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(file) = sources.get(i) else { break };
                    let status = Command::new(&zig)
                        .arg("cc")
                        .args(&flags)
                        .arg("-c")
                        .arg(source.join(file))
                        .arg("-o")
                        .arg(&built[i])
                        .status()
                        .expect("run zig cc");
                    assert!(status.success(), "compile libopus {file}");
                }
            });
        }
    });
    let archive = out.join("libopus.a");
    let _ = std::fs::remove_file(&archive);
    let status = Command::new(&zig)
        .arg("ar")
        .arg("rcs")
        .arg(&archive)
        .args(&built)
        .status()
        .expect("zig ar");
    assert!(status.success(), "archive libopus");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=opus");
    if os != "macos" {
        println!("cargo:rustc-link-lib=m");
    }
}
