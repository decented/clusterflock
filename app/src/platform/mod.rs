//! Platform glue: lifecycle, safe-area insets and layout, raw touch and key
//! input, the data directory and profile, and OS services (clipboard, launch
//! links, wake lock, Wi-Fi multicast lock, room service) and the Bluetooth LE radio ([`ble`]).

#[cfg(target_os = "android")]
pub mod android;
pub mod ble;
#[cfg(target_os = "android")]
pub mod ble_android;
pub mod ble_fake;
pub mod data_dir;
pub mod lan;
pub mod lifecycle;
pub mod os;
pub mod touch;
pub mod viewport;

pub use os::Platform;
pub use touch::{Keys, Touch, TouchPhase, Touches};
pub use viewport::{Insets, Viewport};
pub use winit::keyboard::KeyCode;

/// The hook that binds a socket talking to local peers to the Wi-Fi network, so
/// a VPN or mobile data does not take the route (Android only; `None` elsewhere).
pub fn lan_socket_hook() -> Option<decented_net::LanSocketHook> {
    #[cfg(target_os = "android")]
    {
        Some(android::lan_socket_hook())
    }
    #[cfg(not(target_os = "android"))]
    {
        None
    }
}

/// Whether a VPN is carrying the phone's internet traffic (Android's own answer:
/// any network with TRANSPORT_VPN; true when it cannot be asked). Always false elsewhere.
pub fn vpn_active() -> bool {
    #[cfg(target_os = "android")]
    {
        android::vpn_active()
    }
    #[cfg(not(target_os = "android"))]
    {
        false
    }
}

/// The relay list the account code uses: the library defaults. (Stampede kept a
/// player-editable list in the profile; Cluster Flock has no relay settings yet.)
pub fn relays() -> Vec<String> {
    data_dir::default_relays()
}

/// True on platforms whose primary input is touch.
pub const TOUCH_PLATFORM: bool = cfg!(target_os = "android");

/// Default logical size of the desktop window: a phone held upright.
pub const DESKTOP_SIZE: (f32, f32) = (540.0, 1170.0);

/// The scheme of launch links: `gameflock://join#CODE`.
pub const LINK_SCHEME: &str = "gameflock";

/// The room code in a join link (`gameflock://join#CODE`), upper-cased, or
/// `None` if `link` is not one. Room codes are letters and digits only.
pub fn parse_join_link(link: &str) -> Option<String> {
    let rest = link.trim().strip_prefix(LINK_SCHEME)?.strip_prefix("://")?;
    let (host, code) = rest.split_once('#')?;
    if !host.eq_ignore_ascii_case("join") && !host.eq_ignore_ascii_case("join/") {
        return None;
    }
    let code = code.trim();
    if code.is_empty() || code.len() > 16 || !code.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some(code.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_links() {
        assert_eq!(parse_join_link("gameflock://join#abc234"), Some("ABC234".into()));
        assert_eq!(parse_join_link(" gameflock://join/#KWTQPM "), Some("KWTQPM".into()));
        assert_eq!(parse_join_link("gameflock://join#"), None);
        assert_eq!(parse_join_link("gameflock://other#ABC"), None);
        assert_eq!(parse_join_link("twangthang://join#ABC"), None);
        assert_eq!(parse_join_link("gameflock://join#AB CD"), None);
    }
}
