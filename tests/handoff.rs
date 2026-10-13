//! The hand-off of a Browser sign-in link to the running app, over a Unix socket in a temporary directory.

use futures::StreamExt as _;
use futures::executor::block_on;
use silo_omarchy::handoff::{self, Sent};

fn socket(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("silo-omarchy-test-handoff-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("silo-omarchy.sock")
}

#[test]
fn a_link_sent_to_a_listening_app_arrives_and_is_taken() {
    let path = socket("listening");
    let mut links = handoff::listen(&path).unwrap();
    let link = "org.siloserver.silo:/auth/callback?code=c&state=s&server=server-1&iss=https%3A%2F%2Fsilo.test";
    assert_eq!(handoff::send(&path, link), Sent::Taken);
    assert_eq!(block_on(links.next()).as_deref(), Some(link));
    // A second app does not take the socket from the first one.
    assert_eq!(handoff::listen(&path).err().map(|err| err.kind()), Some(std::io::ErrorKind::AddrInUse));
}

#[test]
fn with_no_listener_nothing_waits() {
    let path = socket("nobody");
    assert_eq!(handoff::send(&path, "org.siloserver.silo:/auth/callback?error=login_failed"), Sent::NothingWaits);
}
