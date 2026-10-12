//! Throwaway prototype for #44. mpv draws into a Wayland subsurface under a GPUI window,
//! and a GPUI button sits over the video. Do not merge it.

use std::ffi::c_void;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Pixels, Render, StatefulInteractiveElement, Styled, Window, WindowBackgroundAppearance, WindowBounds,
    WindowOptions, canvas, div, px, rgb, rgba, size,
};
use khronos_egl as egl;
use libmpv2::Mpv;
use libmpv2::render::{OpenGLInitParams, RenderParam, RenderParamApiType};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};

#[link(name = "wayland-egl")]
unsafe extern "C" {
    fn wl_egl_window_create(surface: *mut c_void, width: i32, height: i32) -> *mut c_void;
    fn wl_egl_window_resize(window: *mut c_void, width: i32, height: i32, dx: i32, dy: i32);
}

type Egl = egl::DynamicInstance<egl::EGL1_4>;

struct WlState;
impl Dispatch<WlRegistry, GlobalListContents> for WlState {
    fn event(_: &mut Self, _: &WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(WlState: WlCompositor);
delegate_noop!(WlState: WlSubcompositor);
delegate_noop!(WlState: WlSubsurface);
delegate_noop!(WlState: WlRegion);
delegate_noop!(WlState: ignore WlSurface);

/// The `wl_display` of GPUI. libwayland and EGL allow its use from any thread.
struct DisplayPtr(*mut c_void);
unsafe impl Send for DisplayPtr {}

/// Physical width, physical height, and buffer scale of the video surface.
type Resize = (i32, i32, i32);

enum Msg {
    Redraw,
    Resize(Resize),
}

/// The video subsurface. GPUI does not know about it.
struct VideoSurface {
    conn: Connection,
    // ponytail: never dispatched; the child surface gets only enter/leave events, which pile up unread.
    _queue: EventQueue<WlState>,
    subsurface: WlSubsurface,
    render: Sender<Msg>,
    last: std::cell::Cell<Option<(i32, i32, Resize)>>,
}

impl VideoSurface {
    fn new(window: &Window, mpv: Arc<Mpv>, path: String) -> Self {
        let RawWindowHandle::Wayland(wh) = HasWindowHandle::window_handle(window).expect("window handle").as_raw() else {
            panic!("this prototype needs Wayland");
        };
        let RawDisplayHandle::Wayland(dh) = window.display_handle().expect("display handle").as_raw() else {
            panic!("this prototype needs Wayland");
        };
        // Undocumented GPUI behavior 1: the handles are the real libwayland wl_display and wl_surface.
        let conn = Connection::from_backend(unsafe { Backend::from_foreign_display(dh.display.as_ptr().cast()) });
        let (globals, queue) = registry_queue_init::<WlState>(&conn).expect("registry");
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).expect("wl_compositor");
        let subcompositor: WlSubcompositor = globals.bind(&qh, 1..=1, ()).expect("wl_subcompositor");
        let parent_id = unsafe { ObjectId::from_ptr(WlSurface::interface(), wh.surface.as_ptr().cast()) }
            .expect("GPUI wl_surface");
        let parent = WlSurface::from_id(&conn, parent_id).expect("wrap GPUI wl_surface");

        let surface = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&surface, &parent, &qh, ());
        subsurface.set_desync();
        // Below the GPUI surface: GPUI leaves a transparent hole and draws its controls over the video.
        subsurface.place_below(&parent);
        // An empty region: the pointer goes through the video to GPUI.
        let region = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        surface.commit();
        conn.flush().expect("flush");

        let render = spawn_render(mpv, DisplayPtr(dh.display.as_ptr()), surface, path);
        Self { conn, _queue: queue, subsurface, render, last: Default::default() }
    }

    /// Moves and resizes the video to the bounds that GPUI laid out. Runs in GPUI paint, so the
    /// position applies with the GPUI commit of the same frame.
    fn place(&self, bounds: Bounds<Pixels>, scale: f32) {
        let x = f32::from(bounds.origin.x).round() as i32;
        let y = f32::from(bounds.origin.y).round() as i32;
        let w = f32::from(bounds.size.width).round().max(1.) as i32;
        let h = f32::from(bounds.size.height).round().max(1.) as i32;
        // ponytail: integer buffer scale; a fractional scale needs wp_viewporter.
        let s = scale.round().max(1.) as i32;
        let next = (x, y, (w * s, h * s, s));
        if self.last.replace(Some(next)) == Some(next) {
            return;
        }
        self.subsurface.set_position(x, y);
        let _ = self.render.send(Msg::Resize(next.2));
        let _ = self.conn.flush();
    }
}

fn get_proc_address(egl: &Arc<Egl>, name: &str) -> *mut c_void {
    egl.get_proc_address(name).map_or(std::ptr::null_mut(), |f| f as *mut c_void)
}

/// The render thread owns the EGL context and the mpv render context.
fn spawn_render(mpv: Arc<Mpv>, display: DisplayPtr, surface: WlSurface, path: String) -> Sender<Msg> {
    let (tx, rx) = channel();
    let wake = tx.clone();
    std::thread::spawn(move || {
        let display = display; // Move the whole Send wrapper, not the raw field.
        let display_ptr = display.0;
        let egl = Arc::new(unsafe { Egl::load_required() }.expect("load libEGL"));
        let display = unsafe { egl.get_display(display_ptr) }.expect("EGL display");
        egl.initialize(display).expect("EGL init");
        egl.bind_api(egl::OPENGL_API).expect("bind OpenGL");
        let attrs = [
            egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8,
            // An alpha channel is required. With an opaque (XRGB) buffer that reaches the bottom-right
            // corner of the window, Hyprland 0.56.2 shows white in place of the video.
            // OPAQUE_BUFFER=1 shows that bug.
            egl::ALPHA_SIZE, if std::env::var_os("OPAQUE_BUFFER").is_some() { 0 } else { 8 },
            egl::SURFACE_TYPE, egl::WINDOW_BIT, egl::RENDERABLE_TYPE, egl::OPENGL_BIT, egl::NONE,
        ];
        let config = egl.choose_first_config(display, &attrs).expect("EGL config").expect("no EGL config");
        let context = egl.create_context(display, config, None, &[egl::NONE]).expect("EGL context");
        let (mut w, mut h) = (16, 16);
        let window = unsafe { wl_egl_window_create(surface.id().as_ptr().cast(), w, h) };
        assert!(!window.is_null(), "wl_egl_window_create");
        let egl_surface = unsafe { egl.create_window_surface(display, config, window, None) }.expect("EGL surface");
        egl.make_current(display, Some(egl_surface), Some(egl_surface), Some(context)).expect("make current");

        let params: [RenderParam<Arc<Egl>>; 3] = [
            RenderParam::ApiType(RenderParamApiType::OpenGl),
            RenderParam::InitParams(OpenGLInitParams { get_proc_address, ctx: egl.clone() }),
            RenderParam::WaylandDisplay(display_ptr),
        ];
        let mut render = mpv.create_render_context(params).expect("mpv render context");
        render.set_update_callback(move || {
            let _ = wake.send(Msg::Redraw);
        });
        mpv.command("loadfile", &[&path]).expect("loadfile");

        let mut frames: u64 = 0;
        while let Ok(first) = rx.recv() {
            // Merge the queued messages: one render for all of them.
            for msg in std::iter::once(first).chain(rx.try_iter()) {
                if let Msg::Resize((nw, nh, scale)) = msg {
                    (w, h) = (nw, nh);
                    unsafe { wl_egl_window_resize(window, w, h, 0, 0) };
                    surface.set_buffer_scale(scale);
                }
            }
            render.render::<Arc<Egl>>(0, w, h, true).expect("mpv render");
            egl.swap_buffers(display, egl_surface).expect("swap buffers");
            render.report_swap();
            frames += 1;
            if frames == 1 || frames % 300 == 0 {
                eprintln!("render: {frames} frames swapped to the subsurface, last size {w}x{h}");
            }
        }
    });
    tx
}

struct Player {
    mpv: Arc<Mpv>,
    video: Rc<VideoSurface>,
    focus: FocusHandle,
    clicks: u32,
}

impl Player {
    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "f" => window.toggle_fullscreen(),
            "space" => {
                let _ = self.mpv.command("cycle", &["pause"]);
            }
            "q" => cx.quit(),
            _ => {}
        }
    }
}

impl Render for Player {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let video = self.video.clone();
        div()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key))
            .size_full()
            .flex()
            .flex_col()
            .text_color(rgb(0xffffff))
            .child(
                div()
                    .h(px(40.))
                    .px_4()
                    .flex()
                    .items_center()
                    .bg(rgb(0x1e1e2e))
                    .child(format!("GPUI bar. Button clicks: {}. Keys: f fullscreen, space pause, q quit.", self.clicks)),
            )
            .child(
                // No background here: GPUI leaves a transparent hole, and the video shows through.
                div()
                    .flex_1()
                    .relative()
                    .child(
                        canvas(|_, _, _| (), move |bounds, _, window, _| video.place(bounds, window.scale_factor()))
                            .absolute()
                            .size_full(),
                    )
                    .child(
                        div()
                            .id("pause")
                            .absolute()
                            .bottom(px(24.))
                            .left(px(24.))
                            .px_4()
                            .py_2()
                            .rounded_md()
                            .bg(rgba(0x000000aa))
                            .hover(|s| s.bg(rgba(0x3366ffcc)))
                            .cursor_pointer()
                            .child("Play / pause (GPUI button)")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.clicks += 1;
                                eprintln!("button: click {}", this.clicks);
                                let _ = this.mpv.command("cycle", &["pause"]);
                                cx.notify();
                            })),
                    ),
            )
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: in-app-video-prototype <video file>");
    let mpv = Arc::new(
        Mpv::with_initializer(|init| {
            init.set_property("vo", "libmpv")?;
            init.set_property("hwdec", "auto-safe")?;
            init.set_property("loop-file", "inf")?;
            init.set_property("terminal", "yes")?;
            init.set_property("msg-level", "all=warn")?;
            Ok(())
        })
        .expect("create mpv"),
    );

    let stats = mpv.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(2));
        let p = |name| stats.get_property::<String>(name).unwrap_or_else(|_| "n/a".into());
        eprintln!(
            "stats: {}x{} hwdec={} fps={} display-fps={} vo-drops={} decoder-drops={} delayed={} mistimed={}",
            p("width"),
            p("height"),
            p("hwdec-current"),
            p("estimated-vf-fps"),
            p("display-fps"),
            p("frame-drop-count"),
            p("decoder-frame-drop-count"),
            p("vo-delayed-frame-count"),
            p("mistimed-frame-count"),
        );
    });

    gpui_kit::application().run(move |cx: &mut App| {
        cx.open_window(
            WindowOptions {
                app_id: Some("silo-in-app-video".into()),
                window_background: WindowBackgroundAppearance::Transparent,
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(760.)), cx))),
                ..Default::default()
            },
            |window, cx| {
                let video = Rc::new(VideoSurface::new(window, mpv.clone(), path.clone()));
                cx.new(|cx| {
                    let focus = cx.focus_handle();
                    window.focus(&focus, cx);
                    if std::env::var_os("TOGGLE_FULLSCREEN").is_some() {
                        // Test aid: go fullscreen and back every 2 s, without a key press.
                        cx.spawn_in(window, async move |this, cx| loop {
                            cx.background_executor().timer(Duration::from_secs(2)).await;
                            if this.update_in(cx, |_, window, _| window.toggle_fullscreen()).is_err() {
                                break;
                            }
                        })
                        .detach();
                    }
                    Player { mpv: mpv.clone(), video, focus, clicks: 0 }
                })
            },
        )
        .expect("open the window");
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.activate(true);
    });
}
