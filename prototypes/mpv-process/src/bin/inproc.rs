// PROTOTYPE, throwaway. libmpv inside the app process. mpv opens its own
// Wayland window with gpu-next; libmpv is only the control channel.
use libmpv2::{Format, Mpv, events::{Event, PropertyData}};

fn main() {
    let url = std::env::args().nth(1).expect("usage: inproc <url>");
    let token = std::env::var("SILO_TOKEN").unwrap_or_else(|_| "proto-token".into());
    println!("[app] pid {}", std::process::id());

    let mpv = Mpv::with_initializer(|init| {
        init.set_option("wayland-app-id", "silo-player")?;
        init.set_option("input-default-bindings", true)?;
        init.set_option("input-vo-keyboard", true)?;
        init.set_option("osc", true)?;
        init.set_option("hwdec", "auto")?;
        if std::env::var_os("PROTO_HEADLESS").is_some() {
            init.set_option("vo", "null")?;
            init.set_option("ao", "null")?;
        }
        Ok(())
    })
    .expect("mpv init");
    // The token lives only in this process memory.
    mpv.set_property("http-header-fields", format!("Authorization: Bearer {token}")).unwrap();
    mpv.observe_property("time-pos", Format::Double, 1).unwrap();
    mpv.command("loadfile", &[&url]).unwrap();

    let mut last = std::time::Instant::now();
    loop {
        match mpv.wait_event(1.0) {
            Some(Ok(Event::PropertyChange { change: PropertyData::Double(t), .. })) => {
                if last.elapsed().as_secs() >= 1 {
                    println!("[mpv] time-pos {t:.1}");
                    last = std::time::Instant::now();
                }
            }
            Some(Ok(Event::EndFile(r))) => println!("[mpv] end-file reason {r}"),
            Some(Ok(Event::Shutdown)) => {
                // The user closed the window or pressed q. The core is gone; a
                // new Mpv handle is needed for the next item.
                println!("[mpv] shutdown");
                break;
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => println!("[mpv] error {e}"),
            None => println!("[app] alive"),
        }
    }
    drop(mpv);
    for _ in 0..3 {
        println!("[app] alive after mpv shutdown");
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
