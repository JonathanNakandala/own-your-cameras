//! Owner-mediated PTZ stop transport. Only the hardware owner polls requests and acknowledges
//! after its stop call returns. No helper thread can outlive the hardware or its ownership lease.
use std::{
    io,
    os::unix::{fs::PermissionsExt, net::UnixDatagram},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::Duration,
};

pub const WAIT: Duration = Duration::from_secs(2);

pub struct Owner {
    socket: UnixDatagram,
    path: PathBuf,
    pending: Option<PathBuf>,
    epoch: Arc<AtomicU32>,
}
impl Owner {
    /// The caller holds the controls lock until this owner is dropped.
    pub fn bind(path: &Path) -> io::Result<Self> {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let socket = UnixDatagram::bind(path)?;
        let owner = Self {
            socket,
            path: path.into(),
            pending: None,
            epoch: Arc::new(AtomicU32::new(0)),
        };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        owner.socket.set_nonblocking(true)?;
        Ok(owner)
    }
    pub fn epoch(&self) -> Arc<AtomicU32> {
        self.epoch.clone()
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Datagram polling has no partially connected clients or blocking accepts.
    pub fn requested(&mut self) -> io::Result<bool> {
        let mut byte = [0; 2];
        match self.socket.recv_from(&mut byte) {
            Ok((1, peer)) if byte[0] == b'S' => {
                let Some(path) = peer.as_pathname() else {
                    return Ok(false);
                };
                self.pending = Some(path.to_owned());
                self.epoch.fetch_add(1, Ordering::AcqRel);
                Ok(true)
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(false),
            Err(e) => Err(e),
            _ => Ok(false),
        }
    }
    pub fn complete(&mut self, result: bool) {
        if let Some(path) = self.pending.take() {
            let _ = self.socket.send_to(if result { b"S" } else { b"E" }, path);
        }
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A bounded caller wait. Timeout/error never means that a stop succeeded.
pub fn request(path: &Path) -> io::Result<()> {
    static SERIAL: AtomicU64 = AtomicU64::new(0);
    let reply_path = path.with_extension(format!(
        "reply-{}-{}",
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let socket = UnixDatagram::bind(&reply_path)?;
    struct Reply(PathBuf);
    impl Drop for Reply {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _reply = Reply(reply_path.clone());
    std::fs::set_permissions(&reply_path, std::fs::Permissions::from_mode(0o600))?;
    socket.set_nonblocking(true)?;
    socket.connect(path)?;
    socket.send(b"S")?;
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(WAIT))?;
    let mut reply = [0; 2];
    if socket.recv(&mut reply)? != 1 || reply[0] != b'S' {
        return Err(io::Error::other("owner could not stop both axes"));
    }
    Ok(())
}
