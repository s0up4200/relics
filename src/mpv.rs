//! The mpv adapter. It is the only module that may hold a `libmpv2::Mpv`
//! (clippy.toml blocks the type everywhere else). The Player window tickets fill it in.
#![allow(clippy::disallowed_types)]
