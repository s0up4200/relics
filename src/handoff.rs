//! The hand-off of a Browser sign-in link between processes. The browser starts a second `relics` with
//! the `org.siloserver.silo:` link. That process sends the link over a Unix socket to the running app and
//! quits (ADR 0004).

use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::channel::mpsc;

/// The scheme of the return link of a Browser sign-in.
pub const SCHEME: &str = "org.siloserver.silo:";

/// The reply of the running app when it took a link.
const TAKEN: &str = "taken";

/// The longest link that the socket takes.
const MAX_LINK: u64 = 8 * 1024;

/// What happened to a link that `send` sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// The running app took the link.
    Taken,
    /// No app listens on the socket.
    NothingWaits,
}

/// `$XDG_RUNTIME_DIR/relics.sock`, or `None` when `$XDG_RUNTIME_DIR` is not set.
pub fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())?;
    Some(PathBuf::from(dir).join("relics.sock"))
}

/// Sends `link` to the app that listens on `path`.
pub fn send(path: &Path, link: &str) -> Sent {
    let reply = || -> io::Result<String> {
        let mut stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.write_all(format!("{link}\n").as_bytes())?;
        stream.shutdown(std::net::Shutdown::Write)?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply)?;
        Ok(reply)
    };
    match reply() {
        Ok(reply) if reply.trim() == TAKEN => Sent::Taken,
        _ => Sent::NothingWaits,
    }
}

/// Listens on `path` and gives each link that arrives. Another app that already listens there keeps the
/// socket, and the call fails with `AddrInUse`. A socket file that nothing listens on is replaced.
pub fn listen(path: &Path) -> io::Result<mpsc::UnboundedReceiver<String>> {
    if UnixStream::connect(path).is_ok() {
        return Err(io::ErrorKind::AddrInUse.into());
    }
    match std::fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let (links, received) = mpsc::unbounded();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if let Some(link) = take(stream)
                && links.unbounded_send(link).is_err()
            {
                return;
            }
        }
    });
    Ok(received)
}

/// Reads one link from `stream` and answers that the app took it. Anything that is not a link gets no answer.
fn take(stream: UnixStream) -> Option<String> {
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_LINK)).read_line(&mut line).ok()?;
    let link = line.trim();
    if !link.starts_with(SCHEME) {
        return None;
    }
    (&stream).write_all(format!("{TAKEN}\n").as_bytes()).ok()?;
    Some(link.to_string())
}
