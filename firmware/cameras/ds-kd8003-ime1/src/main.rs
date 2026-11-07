//! `ds-kd8003-ime1-cam`: the native DS-KD8003-IME1 application. Only the ARM build drives hardware; on any
//! other target the binary exists so the workspace builds and the pure modules are tested.
#[cfg(all(target_arch = "arm", target_os = "linux"))]
mod runtime;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    #[cfg(all(target_arch = "arm", target_os = "linux"))]
    {
        runtime::main(&args);
    }
    #[cfg(not(all(target_arch = "arm", target_os = "linux")))]
    {
        eprintln!(
            "ds-kd8003-ime1-cam {}: host build without hardware support ({} args)",
            env!("CARGO_PKG_VERSION"),
            args.len()
        );
        std::process::exit(2);
    }
}
