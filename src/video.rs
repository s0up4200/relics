//! The video surface of the Player view (ADR 0003): a Wayland child surface under the GPUI surface of the
//! Library window, and a render thread that draws mpv into it with OpenGL. GPUI leaves a transparent hole where
//! the video shows and draws the controls over it. The child surface has an empty input region, so all mouse
//! input goes to GPUI.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::fmt::Display;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use gpui::{Bounds, Pixels, Window};
use khronos_egl as egl;
use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy as _, QueueHandle, delegate_noop};

use crate::mpv;

#[link(name = "wayland-egl")]
unsafe extern "C" {
    fn wl_egl_window_create(surface: *mut c_void, width: i32, height: i32) -> *mut c_void;
    fn wl_egl_window_resize(window: *mut c_void, width: i32, height: i32, dx: i32, dy: i32);
    fn wl_egl_window_destroy(window: *mut c_void);
}

type Egl = egl::DynamicInstance<egl::EGL1_4>;

/// How long a GPUI frame waits for the render thread to draw the video at a new size. mpv can hold a new frame
/// until its display time, so the wait can take one video frame.
const RESIZE_WAIT: Duration = Duration::from_millis(50);

/// The width and the height of the video buffer in pixels, and its buffer scale.
type Size = (i32, i32, i32);

/// The position of the video in the window, and its buffer size.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Place {
    x: i32,
    y: i32,
    size: Size,
}

/// The Wayland events of the child surface. The app needs none of them.
pub struct Events;

impl Dispatch<WlRegistry, GlobalListContents> for Events {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(Events: WlCompositor);
delegate_noop!(Events: WlSubcompositor);
delegate_noop!(Events: WlSubsurface);
delegate_noop!(Events: WlRegion);
delegate_noop!(Events: ignore WlSurface);

/// The `wl_display` of GPUI. libwayland and EGL allow its use from any thread.
struct DisplayPtr(*mut c_void);
// SAFETY: libwayland locks the display for each request, and EGL takes a display from any thread.
unsafe impl Send for DisplayPtr {}

enum Msg {
    /// mpv has a new frame.
    Redraw,
    /// Draw at this size, then answer.
    Resize(Size, Sender<()>),
    Stop,
}

/// The render thread. Stop it with `Surface::stop`.
struct Renderer {
    tx: Sender<Msg>,
    thread: JoinHandle<()>,
}

/// The child surface of the Library window. The window has one for its life. Each mpv handle starts a render
/// thread on it.
pub struct Surface {
    conn: Connection,
    // ponytail: never dispatched. The child surface gets only enter and leave events, which stay unread.
    _queue: EventQueue<Events>,
    display: *mut c_void,
    surface: WlSurface,
    subsurface: WlSubsurface,
    placed: Cell<Option<Place>>,
    /// The subsurface is in sync mode for a resize, until the size stays the same for one GPUI frame.
    synced: Cell<bool>,
    renderer: RefCell<Option<Renderer>>,
}

fn text(err: impl Display) -> String {
    err.to_string()
}

impl Surface {
    /// Makes the child surface under the GPUI surface of `window`. `None` when the window is not on Wayland or
    /// the compositor refuses. Then the video plays in the Player window.
    pub fn new(window: &Window) -> Option<Rc<Self>> {
        Self::make(window)
            .inspect_err(|err| eprintln!("silo-omarchy: no Player view, the video plays in its own window: {err}"))
            .ok()
            .map(Rc::new)
    }

    fn make(window: &Window) -> Result<Self, String> {
        let raw_window = HasWindowHandle::window_handle(window).map_err(text)?.as_raw();
        let (RawWindowHandle::Wayland(handle), RawDisplayHandle::Wayland(display)) =
            (raw_window, window.display_handle().map_err(text)?.as_raw())
        else {
            return Err("the window is not on Wayland".into());
        };
        let display = display.display.as_ptr();
        // ADR 0003: GPUI gives the libwayland wl_display and wl_surface of its window.
        // SAFETY: the display stays open while the window lives, and the surface is a wl_surface of it.
        let conn = Connection::from_backend(unsafe { Backend::from_foreign_display(display.cast()) });
        let (globals, queue) = registry_queue_init::<Events>(&conn).map_err(text)?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).map_err(text)?;
        let subcompositor: WlSubcompositor = globals.bind(&qh, 1..=1, ()).map_err(text)?;
        let parent = unsafe { ObjectId::from_ptr(WlSurface::interface(), handle.surface.as_ptr().cast()) };
        let parent = WlSurface::from_id(&conn, parent.map_err(text)?).map_err(text)?;
        let surface = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&surface, &parent, &qh, ());
        subsurface.set_desync();
        subsurface.place_below(&parent);
        let region = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        surface.commit();
        conn.flush().map_err(text)?;
        Ok(Self {
            conn,
            _queue: queue,
            display,
            surface,
            subsurface,
            placed: Cell::new(None),
            synced: Cell::new(false),
            renderer: RefCell::new(None),
        })
    }

    /// Starts a render thread that draws `video`. It returns when mpv can draw, so the first load can follow.
    pub fn start(&self, video: mpv::Video) -> Result<(), String> {
        self.stop();
        let size = self.placed.get().map_or((16, 16, 1), |place| place.size);
        let (tx, rx) = mpsc::channel();
        let (ready, started) = mpsc::channel();
        let (display, surface, wake) = (DisplayPtr(self.display), self.surface.clone(), tx.clone());
        let thread = std::thread::Builder::new()
            .name("silo-video".into())
            .spawn(move || {
                let display = display;
                run(video, display.0, &surface, size, rx, wake, ready);
            })
            .map_err(text)?;
        match started.recv() {
            Ok(Ok(())) => {
                *self.renderer.borrow_mut() = Some(Renderer { tx, thread });
                Ok(())
            }
            Ok(Err(err)) => {
                thread.join().ok();
                Err(err)
            }
            Err(_) => {
                thread.join().ok();
                Err("the render thread stopped".into())
            }
        }
    }

    /// Stops the render thread and frees the mpv render context and the EGL context. It must run before mpv
    /// closes and before the window closes its Wayland connection.
    pub fn stop(&self) {
        let Some(renderer) = self.renderer.borrow_mut().take() else {
            return;
        };
        renderer.tx.send(Msg::Stop).ok();
        renderer.thread.join().ok();
    }

    /// Removes the last frame, so that the compositor does not draw it under the window. Call it after GPUI
    /// drew a page over the video area, else the desktop shows there for one frame.
    pub fn hide(&self) {
        if self.renderer.borrow().is_some() {
            return;
        }
        if self.synced.replace(false) {
            self.subsurface.set_desync();
        }
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        self.conn.flush().ok();
    }

    /// Moves and resizes the video to `bounds`, the video area that GPUI laid out. Call it in GPUI paint: the
    /// position applies with the GPUI commit of the same frame.
    ///
    /// A new size goes to the render thread, and the frame waits until the video has the new size. The
    /// subsurface is in sync mode meanwhile, so the compositor shows the new video buffer and the new GPUI
    /// buffer together, and the two do not disagree for one frame.
    pub fn place(&self, bounds: Bounds<Pixels>, window: &mut Window) {
        let round = |value: Pixels| f32::from(value).round() as i32;
        // ponytail: an integer buffer scale. A fractional scale needs wp_viewporter.
        let scale = window.scale_factor().round().max(1.) as i32;
        let (width, height) = (round(bounds.size.width).max(1), round(bounds.size.height).max(1));
        let next = Place {
            x: round(bounds.origin.x),
            y: round(bounds.origin.y),
            size: (width * scale, height * scale, scale),
        };
        let last = self.placed.replace(Some(next));
        if last == Some(next) {
            // The size stays: the render thread shows each frame by itself again.
            if self.synced.replace(false) {
                self.subsurface.set_desync();
                self.conn.flush().ok();
            }
            return;
        }
        if last.map(|place| place.size) != Some(next.size)
            && let Some(renderer) = &*self.renderer.borrow()
        {
            if !self.synced.replace(true) {
                self.subsurface.set_sync();
            }
            let (done, answer) = mpsc::channel();
            if renderer.tx.send(Msg::Resize(next.size, done)).is_ok() {
                // A late answer only costs the one frame that this wait tries to save.
                answer.recv_timeout(RESIZE_WAIT).ok();
            }
            // The next frame turns sync mode off when the size stays.
            window.request_animation_frame();
        }
        self.subsurface.set_position(next.x, next.y);
        self.conn.flush().ok();
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The EGL context and the EGL window surface of the render thread.
struct Gl {
    egl: Arc<Egl>,
    display: egl::Display,
    context: egl::Context,
    surface: egl::Surface,
    window: *mut c_void,
}

impl Gl {
    // ponytail: a failure halfway leaks the EGL objects made before it. The app then uses the Player window.
    fn new(wl_display: *mut c_void, surface: &WlSurface, size: Size) -> Result<Self, String> {
        // SAFETY: loads the system libEGL.
        let egl = Arc::new(unsafe { Egl::load_required() }.map_err(text)?);
        // SAFETY: the wl_display is the open display of the window.
        let display = unsafe { egl.get_display(wl_display) }.ok_or("no EGL display")?;
        egl.initialize(display).map_err(text)?;
        egl.bind_api(egl::OPENGL_API).map_err(text)?;
        // The buffer needs an alpha channel. With an opaque buffer that reaches the bottom-right corner of the
        // window, Hyprland 0.56 shows white in place of the video. mpv writes alpha 1, so the video is opaque.
        #[rustfmt::skip]
        let attributes = [
            egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8, egl::ALPHA_SIZE, 8,
            egl::SURFACE_TYPE, egl::WINDOW_BIT, egl::RENDERABLE_TYPE, egl::OPENGL_BIT, egl::NONE,
        ];
        let config = egl.choose_first_config(display, &attributes).map_err(text)?.ok_or("no EGL config")?;
        let context = egl.create_context(display, config, None, &[egl::NONE]).map_err(text)?;
        // SAFETY: the surface is a live wl_surface. The window is destroyed in `drop`.
        let window = unsafe { wl_egl_window_create(surface.id().as_ptr().cast(), size.0, size.1) };
        if window.is_null() {
            return Err("wl_egl_window_create failed".into());
        }
        // SAFETY: the window is a live wl_egl_window.
        let egl_surface = unsafe { egl.create_window_surface(display, config, window, None) }.map_err(text)?;
        egl.make_current(display, Some(egl_surface), Some(egl_surface), Some(context)).map_err(text)?;
        // A swap must not wait for the compositor: it sends no frame callback while the window is hidden, and a
        // swap that waits would stop the render thread and the Player view from closing.
        egl.swap_interval(display, 0).map_err(text)?;
        surface.set_buffer_scale(size.2);
        Ok(Self { egl, display, context, surface: egl_surface, window })
    }

    fn resize(&self, surface: &WlSurface, size: Size) {
        // SAFETY: the window is a live wl_egl_window. The next swap applies the size and the scale.
        unsafe { wl_egl_window_resize(self.window, size.0, size.1, 0, 0) };
        surface.set_buffer_scale(size.2);
    }
}

impl Drop for Gl {
    fn drop(&mut self) {
        self.egl.make_current(self.display, None, None, None).ok();
        self.egl.destroy_surface(self.display, self.surface).ok();
        self.egl.destroy_context(self.display, self.context).ok();
        // SAFETY: no EGL surface uses the window any more.
        unsafe { wl_egl_window_destroy(self.window) };
    }
}

fn get_proc_address(egl: &Arc<Egl>, name: &str) -> *mut c_void {
    egl.get_proc_address(name).map_or(std::ptr::null_mut(), |f| f as *mut c_void)
}

/// The render thread. It owns the EGL context and the mpv render context, and draws a frame on each message.
/// It drops the render context before the EGL context, and both before `video`.
fn run(
    video: mpv::Video,
    wl_display: *mut c_void,
    surface: &WlSurface,
    mut size: Size,
    rx: Receiver<Msg>,
    wake: Sender<Msg>,
    ready: Sender<Result<(), String>>,
) {
    let gl = match Gl::new(wl_display, surface, size) {
        Ok(gl) => gl,
        Err(err) => return drop(ready.send(Err(format!("EGL: {err}")))),
    };
    let mut context = match video.render_context(get_proc_address, gl.egl.clone(), wl_display) {
        Ok(context) => context,
        Err(err) => return drop(ready.send(Err(format!("mpv render context: {err}")))),
    };
    context.set_update_callback(move || drop(wake.send(Msg::Redraw)));
    ready.send(Ok(())).ok();
    while let Ok(first) = rx.recv() {
        // One frame for all the queued messages.
        let mut answers = Vec::new();
        for msg in std::iter::once(first).chain(rx.try_iter()) {
            match msg {
                Msg::Redraw => {}
                Msg::Resize(next, done) => {
                    if next != size {
                        size = next;
                        gl.resize(surface, size);
                    }
                    answers.push(done);
                }
                Msg::Stop => return,
            }
        }
        if let Err(err) = context.render::<Arc<Egl>>(0, size.0, size.1, true) {
            eprintln!("silo-omarchy: mpv cannot draw the video: {err}");
        }
        if let Err(err) = gl.egl.swap_buffers(gl.display, gl.surface) {
            // The thread ends, and the Player view shows no more frames. It must not panic.
            return eprintln!("silo-omarchy: the video cannot show: {err}");
        }
        context.report_swap();
        for done in answers {
            done.send(()).ok();
        }
    }
}
