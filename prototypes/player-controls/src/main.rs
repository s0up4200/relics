// PROTOTYPE, throwaway. Player controls for silo-omarchy#8.
//
//   cargo run -- osc   built-in mpv on-screen controller, prompt as OSD text
//   cargo run -- lua   custom Lua controller (silo-controls.lua), themed
//
// SKIP_MODE=ask|always|never (default ask), like playback.intro_skip_mode.
// In both variants the skip state machine lives here, in the app. The
// controller only draws the prompt and sends back "silo-action".
use libmpv2::{Format, Mpv, events::{Event, PropertyData}};
use std::time::{Duration, Instant};

struct Marker { kind: &'static str, start: f64, end: f64 }

// Fake marker_segments for the 120 s sample. Credits run to the end of file.
const MARKERS: &[Marker] = &[
    Marker { kind: "recap", start: 3.0, end: 8.0 },
    Marker { kind: "intro", start: 15.0, end: 30.0 },
    Marker { kind: "credits", start: 105.0, end: 120.0 },
];

struct Prompt { label: String, marker: usize, undo: bool, until: Instant }

fn main() {
    let variant = std::env::args().nth(1).unwrap_or_else(|| "lua".into());
    let mode = std::env::var("SKIP_MODE").unwrap_or_else(|_| "ask".into());
    let dir = std::env::current_dir().unwrap();
    println!("[app] variant {variant}, skip mode {mode}");

    // Markers as chapters, so the built-in OSC shows ticks on the seek bar.
    let mut ch = String::from(";FFMETADATA1\n");
    for m in MARKERS {
        ch += &format!("[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={}\n",
            (m.start * 1000.0) as i64, (m.end * 1000.0) as i64, m.kind);
    }
    std::fs::write("chapters.txt", ch).unwrap();

    let mpv = Mpv::with_initializer(|init| {
        init.set_option("wayland-app-id", "silo-player")?;
        init.set_option("input-default-bindings", true)?;
        init.set_option("input-vo-keyboard", true)?;
        init.set_option("hwdec", "auto")?;
        init.set_option("keep-open", "yes")?;
        init.set_option("chapters-file", "chapters.txt")?;
        init.set_option("sub-files", "sample.eng.srt:sample.nor.srt")?;
        if std::env::var_os("PROTO_HEADLESS").is_some() {
            init.set_option("vo", "null")?;
            init.set_option("ao", "null")?;
            init.set_option("start", "12")?;
        }
        if variant == "lua" {
            init.set_option("osc", false)?;
            init.set_option("osd-bar", false)?;
            init.set_option("scripts", dir.join("silo-controls.lua").to_str().unwrap())?;
        } else {
            init.set_option("osc", true)?;
        }
        Ok(())
    })
    .expect("mpv init");

    if variant == "osc" {
        // ENTER is playlist-next by default. Take it for the skip prompt.
        mpv.command("keybind", &["ENTER", "script-message silo-action"]).unwrap();
    }
    mpv.observe_property("time-pos", Format::Double, 1).unwrap();
    mpv.command("loadfile", &["sample.mkv"]).unwrap();

    let mut prompt: Option<Prompt> = None;
    let mut inside: Option<usize> = None;
    let mut pos = 0.0;

    let show = |mpv: &Mpv, p: &Option<Prompt>| {
        let label = p.as_ref().map_or("", |p| p.label.as_str());
        println!("[app] prompt {label:?}");
        if variant == "lua" {
            mpv.command("script-message", &["silo-prompt", label]).unwrap();
        } else if !label.is_empty() {
            mpv.command("show-text", &[&format!("{label}   [Enter]"), "5000"]).unwrap();
        } else {
            mpv.command("show-text", &["", "1"]).unwrap();
        }
    };

    loop {
        match mpv.wait_event(0.25) {
            Some(Ok(Event::PropertyChange { change: PropertyData::Double(t), .. })) => {
                pos = t;
                let now = MARKERS.iter().position(|m| t >= m.start && t < m.end - 0.5);
                if now != inside {
                    inside = now;
                    prompt = None;
                    if let Some(i) = now {
                        let m = &MARKERS[i];
                        let at_end = m.kind == "credits" && m.end >= 119.0;
                        if at_end {
                            prompt = Some(Prompt { label: "Next episode".into(), marker: i, undo: false, until: Instant::now() + Duration::from_secs(15) });
                        } else if mode == "always" {
                            mpv.command("seek", &[&m.end.to_string(), "absolute"]).unwrap();
                            prompt = Some(Prompt { label: format!("Watch {}", m.kind), marker: i, undo: true, until: Instant::now() + Duration::from_secs(5) });
                        } else if mode == "ask" {
                            prompt = Some(Prompt { label: format!("Skip {}", m.kind), marker: i, undo: false, until: Instant::now() + Duration::from_secs(5) });
                        }
                    }
                    show(&mpv, &prompt);
                }
            }
            Some(Ok(Event::ClientMessage(msg))) => {
                println!("[mpv] client message {msg:?}");
                match msg.as_slice() {
                    ["silo-ready"] => {
                        let (a, f, b) = omarchy_colors();
                        mpv.command("script-message", &["silo-theme", &a, &f, &b]).unwrap();
                        let marks: Vec<String> = MARKERS.iter().map(|m| format!("{}:{}:{}", m.kind, m.start, m.end)).collect();
                        mpv.command("script-message", &["silo-markers", &marks.join(",")]).unwrap();
                    }
                    ["silo-action"] => if let Some(p) = prompt.take() {
                        let m = &MARKERS[p.marker];
                        if p.label == "Next episode" {
                            println!("[app] would open next episode at pos {pos:.1}");
                        } else if p.undo {
                            mpv.command("seek", &[&m.start.to_string(), "absolute"]).unwrap();
                        } else {
                            mpv.command("seek", &[&m.end.to_string(), "absolute"]).unwrap();
                        }
                        show(&mpv, &None);
                    },
                    _ => {}
                }
            }
            Some(Ok(Event::Shutdown)) => { println!("[mpv] shutdown"); break; }
            Some(Ok(_)) | None => {}
            Some(Err(e)) => println!("[mpv] error {e}"),
        }
        if prompt.as_ref().is_some_and(|p| Instant::now() > p.until) {
            prompt = None;
            show(&mpv, &prompt);
        }
    }
}

// Accent, foreground and background from the live Omarchy theme.
fn omarchy_colors() -> (String, String, String) {
    let path = std::env::var("HOME").unwrap() + "/.local/state/omarchy/current/theme/colors.toml";
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let get = |k: &str, d: &str| text.lines()
        .find_map(|l| l.strip_prefix(k)?.trim_start().strip_prefix('=').map(|v| v.trim().trim_matches('"').to_string()))
        .unwrap_or_else(|| d.into());
    (get("accent", "#7aa2f7"), get("foreground", "#ffffff"), get("background", "#000000"))
}
