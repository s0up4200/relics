//! Test helper: moves the pointer to a layout position, clicks the left button, and moves the
//! pointer back. Usage: click <x> <y> <back x> <back y> <layout width> <layout height>

use std::time::Duration;

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_pointer::ButtonState;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1;
use wayland_protocols_wlr::virtual_pointer::v1::client::zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1;

struct State;
impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(State: ZwlrVirtualPointerManagerV1);
delegate_noop!(State: ZwlrVirtualPointerV1);

const BTN_LEFT: u32 = 0x110;

fn main() {
    let a: Vec<u32> = std::env::args().skip(1).map(|v| v.parse().expect("a number")).collect();
    assert_eq!(a.len(), 6, "usage: click <x> <y> <back x> <back y> <layout width> <layout height>");
    let conn = Connection::connect_to_env().expect("connect to Wayland");
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = queue.handle();
    let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).expect("zwlr_virtual_pointer_manager_v1");
    let pointer = manager.create_virtual_pointer(None, &qh, ());
    let mut step = |time: u32, f: &dyn Fn(u32)| {
        f(time);
        pointer.frame();
        queue.roundtrip(&mut State).expect("roundtrip");
        std::thread::sleep(Duration::from_millis(150));
    };
    step(1, &|t| pointer.motion_absolute(t, a[0], a[1], a[4], a[5]));
    step(2, &|t| pointer.button(t, BTN_LEFT, ButtonState::Pressed));
    step(3, &|t| pointer.button(t, BTN_LEFT, ButtonState::Released));
    step(4, &|t| pointer.motion_absolute(t, a[2], a[3], a[4], a[5]));
    pointer.destroy();
    conn.flush().expect("flush");
}
