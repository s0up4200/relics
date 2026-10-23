use std::sync::Arc;

use gpui::{
    AppContext as _, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowOptions, px, size,
};
use relics::assets::Assets;
use relics::handoff;
use relics::library::LibraryWindow;
use reqwest_client::ReqwestClient;

fn main() {
    relics::startup::mark("main");
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "--version") {
        println!("{}", relics::BUILD_IDENTITY);
        return;
    }
    eprintln!("{}", relics::BUILD_IDENTITY);
    // The browser starts a second process with the return link of a Browser sign-in. It gives the link to the
    // running app and quits. When no app waits, the sign-in secret was only in the closed window.
    let link = std::env::args().nth(1).filter(|arg| arg.starts_with(handoff::SCHEME));
    let socket = handoff::socket_path();
    if let (Some(link), Some(socket)) = (&link, &socket)
        && handoff::send(socket, link) == handoff::Sent::Taken
    {
        return;
    }
    let links = socket.and_then(|socket| {
        handoff::listen(&socket)
            .inspect_err(|err| eprintln!("relics: cannot listen on {}: {err}", socket.display()))
            .ok()
    });
    gpui_kit::application().with_assets(Assets).run(move |cx| {
        gpui_omarchy::init(cx);
        relics::text_input::init(cx);
        relics::theme::keep_readable(cx);
        let user_agent = concat!("relics/", env!("CARGO_PKG_VERSION"));
        let http = ReqwestClient::proxy_and_user_agent(gpui::http_client::read_proxy_from_env(), user_agent)
            .expect("create the HTTP client");
        cx.set_http_client(Arc::new(http));
        cx.open_window(
            WindowOptions {
                app_id: Some("relics".into()),
                titlebar: Some(TitlebarOptions { title: Some("Relics".into()), ..Default::default() }),
                // The Player view leaves a transparent hole where the video shows (ADR 0003).
                window_background: WindowBackgroundAppearance::Transparent,
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1400.), px(900.)), cx))),
                ..Default::default()
            },
            move |window, cx| cx.new(|cx| LibraryWindow::new(links, link, window, cx)),
        )
        .expect("open the Library window");
        cx.activate(true);
    });
}
