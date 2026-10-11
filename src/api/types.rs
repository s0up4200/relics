//! Hand-written API types. `wire.rs` decodes each enum of the API as a plain string,
//! so a newer server can add a value. The open enums here turn such a string into a
//! known variant, or into `Unknown` with the value.

use serde::{Deserialize, Serialize};

/// The error body (`application/problem+json`) of a refused request.
#[derive(Debug, Clone, Deserialize)]
pub struct Problem {
    /// A URI. Its last path segment is the problem code.
    #[serde(rename = "type")]
    pub type_: String,
    pub title: String,
    pub status: u16,
    #[serde(default)]
    pub detail: String,
}

impl Problem {
    /// The problem code, for example `profile_verification_required`.
    pub fn code(&self) -> &str {
        self.type_.rsplit('/').next().unwrap_or_default()
    }
}

macro_rules! open_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $wire:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
        #[serde(from = "String", into = "String")]
        pub enum $name {
            $($variant,)+
            /// A value that this version of the app does not know.
            Unknown(String),
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                match value.as_str() {
                    $($wire => Self::$variant,)+
                    _ => Self::Unknown(value),
                }
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> String {
                match value {
                    $($name::$variant => $wire.to_string(),)+
                    $name::Unknown(value) => value,
                }
            }
        }
    };
}

open_enum! {
    /// `playback_plan.delivery` of a playback decision.
    Delivery {
        OriginalHttp = "original_http",
        ServerRemuxProgressive = "server_remux_progressive",
        ServerRemuxHls = "server_remux_hls",
        ServerTranscodeHls = "server_transcode_hls",
    }
}

open_enum! {
    /// `stream.protocol` of a playback plan.
    StreamProtocol { HttpProgressive = "http_progressive", Hls = "hls" }
}

open_enum! {
    /// `stream.header_refresh` of a playback plan.
    HeaderRefresh { None = "none", Session = "session" }
}

open_enum! {
    /// `timeline.seek_restoration` of a playback plan.
    SeekRestoration { PlayerPosition = "player_position", SourcePosition = "source_position" }
}

open_enum! {
    /// `status` of a device sign-in poll.
    DeviceLoginStatus {
        Pending = "pending",
        Approved = "approved",
        Denied = "denied",
        Consumed = "consumed",
        Canceled = "canceled",
        Expired = "expired",
    }
}

open_enum! {
    /// The kind of a Marker.
    MarkerKind { Intro = "intro", Credits = "credits", Recap = "recap", Preview = "preview" }
}
