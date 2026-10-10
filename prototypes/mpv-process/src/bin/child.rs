// PROTOTYPE, throwaway. mpv as a child process, controlled over JSON IPC on a
// socketpair (no socket path on disk, no token on the command line).
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsRawFd;
use std::os::unix::{net::UnixStream, process::CommandExt};
use std::process::Command;

fn main() {
    let url = std::env::args().nth(1).expect("usage: child <url>");
    let token = std::env::var("SILO_TOKEN").unwrap_or_else(|_| "proto-token".into());
    println!("[app] pid {}", std::process::id());

    let (mut ours, theirs) = UnixStream::pair().unwrap();
    let fd = theirs.as_raw_fd();
    let mut cmd = Command::new("mpv");
    cmd.args([
        "--idle=yes",
        "--no-config",
        "--input-ipc-client=fd://3",
        "--wayland-app-id=silo-player",
        "--hwdec=auto",
        // Without this, yt-dlp retries a failed URL without the auth header.
        "--ytdl=no",
        "--msg-level=all=warn",
    ]);
    if std::env::var_os("PROTO_HEADLESS").is_some() {
        cmd.args(["--vo=null", "--ao=null"]);
    }
    unsafe {
        cmd.pre_exec(move || {
            // Kill mpv when the app dies, so no orphan keeps playing.
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            libc::dup2(fd, 3);
            Ok(())
        });
    }
    let mut child = cmd.spawn().expect("spawn mpv");
    drop(theirs);
    println!("[app] mpv pid {}", child.id());

    let header = format!("Authorization: Bearer {token}");
    for msg in [
        serde_json::json!({"command": ["set_property", "http-header-fields", [header]], "request_id": 1}),
        serde_json::json!({"command": ["observe_property", 1, "time-pos"]}),
        serde_json::json!({"command": ["loadfile", url], "request_id": 2}),
    ] {
        writeln!(ours, "{msg}").unwrap();
    }

    let reader = BufReader::new(ours.try_clone().unwrap());
    std::thread::spawn(move || {
        let mut last = std::time::Instant::now();
        for line in reader.lines() {
            let Ok(line) = line else { break };
            let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
            match v["event"].as_str() {
                Some("property-change") => {
                    if let Some(t) = v["data"].as_f64()
                        && last.elapsed().as_secs() >= 1
                    {
                        println!("[mpv] time-pos {t:.1}");
                        last = std::time::Instant::now();
                    }
                }
                Some(_) | None => println!("[mpv] {line}"),
            }
        }
        println!("[mpv] socket closed");
    });

    loop {
        if let Ok(Some(status)) = child.try_wait() {
            println!("[app] mpv exited: {status}; app still alive");
            for _ in 0..3 {
                std::thread::sleep(std::time::Duration::from_secs(1));
                println!("[app] alive");
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
        println!("[app] alive");
    }
}
