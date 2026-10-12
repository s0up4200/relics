use std::borrow::Cow;
use std::sync::Arc;

use gpui::{
    AppContext as _, AssetSource, Bounds, SharedString, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowOptions, px, size,
};
use reqwest_client::ReqwestClient;
use silo_omarchy::library::LibraryWindow;

// The sidebar and Player view icons that the default gpui-kit bundle does not have.
gpui_kit::assets::icon_assets!(
    SidebarIcons,
    [House, Film, Tv, Layers, Bookmark, RotateCcwClock, Minimize, Volume2, VolumeX, AudioLines, Captions]
);

/// The default gpui-kit assets and the sidebar icons.
struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        match SidebarIcons.load(path)? {
            Some(icon) => Ok(Some(icon)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut list = gpui_kit::assets::Assets.list(path)?;
        list.extend(SidebarIcons.list(path)?);
        Ok(list)
    }
}

fn main() {
    gpui_kit::application().with_assets(Assets).run(|cx| {
        gpui_omarchy::init(cx);
        let user_agent = concat!("silo-omarchy/", env!("CARGO_PKG_VERSION"));
        let http = ReqwestClient::proxy_and_user_agent(gpui::http_client::read_proxy_from_env(), user_agent)
            .expect("create the HTTP client");
        cx.set_http_client(Arc::new(http));
        cx.open_window(
            WindowOptions {
                app_id: Some("silo-omarchy".into()),
                titlebar: Some(TitlebarOptions { title: Some("Silo".into()), ..Default::default() }),
                // The Player view leaves a transparent hole where the video shows (ADR 0003).
                window_background: WindowBackgroundAppearance::Transparent,
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1400.), px(900.)), cx))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| LibraryWindow::new(window, cx)),
        )
        .expect("open the Library window");
        cx.activate(true);
    });
}
