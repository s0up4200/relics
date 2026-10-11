use std::sync::Arc;

use gpui::{AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use reqwest_client::ReqwestClient;
use silo_omarchy::library::LibraryWindow;

fn main() {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(|cx| {
        gpui_omarchy::init(cx);
        let user_agent = concat!("silo-omarchy/", env!("CARGO_PKG_VERSION"));
        let http = ReqwestClient::proxy_and_user_agent(gpui::http_client::read_proxy_from_env(), user_agent)
            .expect("create the HTTP client");
        cx.set_http_client(Arc::new(http));
        cx.open_window(
            WindowOptions {
                app_id: Some("silo-omarchy".into()),
                titlebar: Some(TitlebarOptions { title: Some("Silo".into()), ..Default::default() }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1400.), px(900.)), cx))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| LibraryWindow::new(window, cx)),
        )
        .expect("open the Library window");
        cx.activate(true);
    });
}
