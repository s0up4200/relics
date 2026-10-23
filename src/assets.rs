//! The assets of the app: the default gpui-kit bundle, the sidebar and Player view icons, and the Tailscale logo.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

// The sidebar and Player view icons that the default gpui-kit bundle does not have.
gpui_kit::assets::icon_assets!(
    SidebarIcons,
    [
        House,
        Film,
        Tv,
        Layers,
        Bookmark,
        RotateCcwClock,
        LibraryBig,
        Minimize,
        Volume2,
        VolumeX,
        AudioLines,
        Captions,
        Gauge,
        Info,
        ListVideo,
        SkipForward,
        KeyRound,
        MonitorSmartphone
    ]
);

/// The Tailscale logo of the Network sign-in button. Lucide has no brand logos.
const TAILSCALE: &str = "icons/tailscale.svg";

/// The default gpui-kit assets, the sidebar icons and the Tailscale logo.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        if path == TAILSCALE {
            return Ok(Some(Cow::Borrowed(include_bytes!("../data/tailscale.svg"))));
        }
        match SidebarIcons.load(path)? {
            Some(icon) => Ok(Some(icon)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut list = gpui_kit::assets::Assets.list(path)?;
        list.extend(SidebarIcons.list(path)?);
        list.extend(TAILSCALE.starts_with(path).then(|| TAILSCALE.into()));
        Ok(list)
    }
}
