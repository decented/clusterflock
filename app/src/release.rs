//! Cluster Flock's side of in-app updates and More games (spec 20): the values
//! and the wiring, nothing else. The rules (the signed kind-30063 release event,
//! the pinned key, the canonical Blossom URLs, hash, size and certificate checks,
//! the `Updater` and `Catalog` state machines, the Android installer) are
//! nostr-release-kit's; `tools/release/release.json` configures the kit's
//! publisher for the same channel, package and key.
//!
//! One function builds the kit's `Config` ([`update_config`]) and one the
//! `CatalogConfig` ([`catalog_config`]); the engine and the pin tests both call
//! them, so what the tests pin is what ships.

use crate::games::CATALOG_ENTRIES;
use crate::nostr_fetch::AppFetcher;
use nostr_release_kit::{Catalog, CatalogConfig, Config, Updater};
use std::path::Path;

/// The `d` tag of this app's Android channel.
pub const RELEASE_CHANNEL: &str = "gameflock-android-apk";
/// Read for the release event; also what the publisher writes to (`tools/release/release.json`).
pub const RELEASE_RELAYS: [&str; 3] = ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net"];
/// The release key's public half (hex), Cluster Flock's own (one release key per game),
/// minted 2026-11-09 with the publisher's `--init-key`; the publisher refuses to sign
/// with any other key. Rotating it means shipping a new APK by hand: there is no
/// in-band rotation.
pub const RELEASE_PUBKEY_HEX: &str = "01b0f270e290f5cd48aa537392de04e4b22124c16d696de96158395a60343b0f";
/// This app's own package: the one installer's "owner" while Cluster Flock updates itself.
pub const OWN_PACKAGE: &str = "org.decented.gameflock";
/// The name used in sentences ("Allow installs from Cluster Flock ...").
pub const APP_NAME: &str = "Cluster Flock";
/// The file next to `profile.json`: last check time and the best verified release event.
pub const STATE_FILE: &str = "update.json";

/// The kit's `Config` for this app. `state_dir`: where `update.json` lives (next to
/// `profile.json`); `None` keeps the state in memory only (screenshots and tests: their
/// staged offers are throwaway-signed and must never reach a file).
pub fn update_config(state_dir: Option<&Path>) -> Config {
    // The versionCode is derived from the workspace version (build-apk.sh stamps the same formula);
    // `the_config_the_engine_builds_is_flocks_own` fails the build if it ever has none.
    let mut config = Config::new(RELEASE_CHANNEL, OWN_PACKAGE, APP_NAME, env!("CARGO_PKG_VERSION"))
        .expect("the workspace version has a versionCode and the constants are valid")
        .with_release_pubkey_hex(RELEASE_PUBKEY_HEX)
        .expect("RELEASE_PUBKEY_HEX is a valid x-only key");
    config.relays = RELEASE_RELAYS.iter().map(|s| s.to_string()).collect();
    // "v0.2.0 . <build>" on the Settings card.
    config.version_label = Some(crate::version_label());
    match state_dir {
        Some(dir) => config.with_state_file(dir.join(STATE_FILE)),
        None => config,
    }
}

/// The in-app updater, reading the relays through [`AppFetcher`].
pub fn updater(state_dir: Option<&Path>) -> Updater {
    Updater::new(update_config(state_dir), Box::new(AppFetcher))
}

/// The kit's `CatalogConfig` for More games: [`CATALOG_ENTRIES`] and the release relays.
pub fn catalog_config() -> CatalogConfig {
    let mut config = CatalogConfig::new(CATALOG_ENTRIES.to_vec());
    config.relays = RELEASE_RELAYS.iter().map(|s| s.to_string()).collect();
    config
}

/// The More games screen's data, reading the relays through [`AppFetcher`].
pub fn catalog() -> Catalog {
    Catalog::new(catalog_config(), Box::new(AppFetcher))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::util::JsonUtil;
    use nostr::{Event, PublicKey};
    use nostr_release_kit::{parse_release, version_code_of, Verdict};

    /// Cluster Flock's signing certificate, as the publisher pins it (`tools/release/release.json`).
    const EXPECTED_CERT: &str = "46fd4f7a80b700cd7d951c04e38fa3344405759d53d161de3e4dd06120fb3ace";

    #[test]
    fn the_shipped_constants_are_flocks_own() {
        assert_eq!(RELEASE_CHANNEL, "gameflock-android-apk");
        assert_eq!(RELEASE_RELAYS, ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net"]);
        assert_eq!(OWN_PACKAGE, "org.decented.gameflock");
        assert_eq!(APP_NAME, "Cluster Flock");
        assert_eq!(RELEASE_PUBKEY_HEX, "01b0f270e290f5cd48aa537392de04e4b22124c16d696de96158395a60343b0f");
        // Twang Thang's release key is not Cluster Flock's: it must never be pinned here.
        assert_ne!(RELEASE_PUBKEY_HEX, "2cc65a54774d9c1d6fde7e98255dbe1064940facb10dbc3e49f63300a27178c7");
        assert!(CATALOG_ENTRIES.iter().all(|e| e.release_pubkey != Some(RELEASE_PUBKEY_HEX) && e.channel != RELEASE_CHANNEL));
    }

    #[test]
    fn the_pinned_key_is_a_real_key() {
        assert!(PublicKey::from_hex(RELEASE_PUBKEY_HEX).is_ok());
        assert!(RELEASE_PUBKEY_HEX.bytes().any(|b| b != b'0'));
    }

    #[test]
    fn the_config_the_engine_builds_is_flocks_own() {
        let dir = std::path::Path::new("some/dir");
        let c = update_config(Some(dir));
        assert_eq!((c.channel.as_str(), c.own_package.as_str(), c.app_name.as_str()), (RELEASE_CHANNEL, OWN_PACKAGE, APP_NAME));
        assert_eq!(c.release_pubkey, Some(PublicKey::from_hex(RELEASE_PUBKEY_HEX).unwrap()));
        assert_eq!(c.relays, RELEASE_RELAYS.map(String::from).to_vec());
        assert_eq!(c.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(c.version_label, Some(crate::version_label()));
        assert_eq!(c.check_interval, std::time::Duration::from_secs(6 * 3600));
        assert_eq!(c.fetch_timeout, std::time::Duration::from_secs(8));
        assert_eq!(c.state_file, Some(dir.join("update.json")));
        assert_eq!(update_config(None).state_file, None, "screenshots and tests keep nothing on disk");
        // The installed versionCode is the workspace version's, as build-apk.sh stamps it.
        assert_eq!(updater(None).installed_code(), version_code_of(env!("CARGO_PKG_VERSION")).unwrap());
        assert_eq!(c.version_code, updater(None).installed_code());
        assert!(updater(None).installed_code() > 0 && updater(None).configured());
    }

    #[test]
    fn the_catalog_config_carries_the_entries_and_the_release_relays() {
        let c = catalog_config();
        assert_eq!(c.entries, CATALOG_ENTRIES.to_vec());
        assert_eq!(c.relays, RELEASE_RELAYS.map(String::from).to_vec());
        let cat = catalog();
        assert_eq!(cat.entries().len(), 2);
        // Every entry names a real package that is not this app's own, and none repeats.
        assert_eq!(c.validate(OWN_PACKAGE), Ok(()));
        let filters = cat.filters();
        assert_eq!(filters.len(), 2, "one filter per ready entry");
        let json = filters.iter().map(|f| f.as_json()).collect::<Vec<_>>().join(" ");
        assert!(json.contains("2cc65a54774d9c1d6fde7e98255dbe1064940facb10dbc3e49f63300a27178c7"), "{json}");
        assert!(json.contains("30063") && json.contains("twangthang-android-apk") && json.contains("stampede-android-apk"), "{json}");
    }

    /// `tools/release/release.json` configures the kit's publisher; it and the Rust constants
    /// must say the same thing, or the publisher would sign what the app does not trust.
    #[test]
    fn the_publisher_config_matches_the_apps_constants() {
        let json: serde_json::Value = serde_json::from_str(include_str!("../../tools/release/release.json")).unwrap();
        let c = update_config(None);
        assert_eq!(json["channel"], c.channel.as_str());
        assert_eq!(json["package"], c.own_package.as_str());
        assert_eq!(json["release_pubkey"], c.release_pubkey.unwrap().to_hex().as_str());
        assert_eq!(json["relays"], serde_json::json!(c.relays));
        assert_eq!(json["expected_cert_sha256"], EXPECTED_CERT);
        assert_eq!(json["version_source"], "semver", "the versionCode is derived the way build-apk.sh derives it");
        // The key FILE is never read here: only the names the publisher looks it up by.
        assert_eq!(json["key_file_env"], "FLOCK_RELEASE_KEY_FILE");
    }

    /// The kit's own publisher output, signed with a THROWAWAY key whose secret was discarded, on
    /// this app's real channel: it is what a Cluster Flock release event looks like to the app.
    /// Never commit an event signed by the real release key (it would be a publishable announcement).
    const EVENT: &str = r#"{"kind":30063,"created_at":1791886220,"tags":[["d","gameflock-android-apk"],["version","0.1.0"],["version_code","1000"],["x","50d858e0985ecc7f60418aaf0cc5ab587f42c2570a884095a9e8ccacd0f6545c"],["size","1048576"],["cert","cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"],["url","https://blossom.example.com/50d858e0985ecc7f60418aaf0cc5ab587f42c2570a884095a9e8ccacd0f6545c.apk"]],"content":"First in-app update release.","pubkey":"aa70f4d73e50a2242408fce300fead275be5c4a36b39693062116b94d4457369","id":"2fee13a7f8bf484bc894607e6fa4ef72b8803ace7edfc043a64d6d1289382220","sig":"7b007f56a6478b97bbd555d7229fdd4d378ba15fecca0e23596208c90a9d888c6c9d666cc2eaa261f0a7f46f616b336a3c3b474f9c2ab429b21e1d4a21f15850"}"#;
    const TEST_PUBKEY: &str = "aa70f4d73e50a2242408fce300fead275be5c4a36b39693062116b94d4457369";

    #[test]
    fn an_event_on_flocks_channel_is_judged_through_the_real_config() {
        let ev: Event = serde_json::from_str(EVENT).unwrap();
        let test_key = PublicKey::from_hex(TEST_PUBKEY).unwrap();
        // The app's real channel accepts what the publisher signs, for the key it was signed by...
        let r = parse_release(&ev, &test_key, RELEASE_CHANNEL).expect("the app accepts what the publisher signs");
        assert_eq!((r.version.as_str(), r.version_code, r.size), ("0.1.0", 1000, 1_048_576));
        assert_eq!(r.notes, "First in-app update release.");
        // ...and the production key does not trust it: nothing is offered, nothing remembered.
        let mut prod = updater(None);
        prod.set_installed(500);
        prod.apply_events(std::slice::from_ref(&ev), 5);
        assert!(matches!(prod.verdict(), Verdict::Unknown) && prod.pill().is_none());
        assert!(prod.due(5), "nothing was recorded");
        // With the throwaway key pinned in the otherwise real Config, the same event is an offer for
        // an older install, and not one for an install that already has it.
        let mut cfg = update_config(None);
        cfg.release_pubkey = Some(test_key);
        let mut u = Updater::new(cfg.clone(), Box::new(AppFetcher));
        u.set_installed(500);
        u.apply_events(std::slice::from_ref(&ev), 5);
        assert!(matches!(u.verdict(), Verdict::Offer(o) if o.version_code == 1000));
        assert_eq!(u.pill().as_deref(), Some("Update to v0.1.0"));
        let mut current = Updater::new(cfg, Box::new(AppFetcher));
        current.set_installed(1000);
        current.apply_events(std::slice::from_ref(&ev), 5);
        assert!(matches!(current.verdict(), Verdict::UpToDate));
    }

    #[test]
    fn the_permission_line_names_cluster_flock() {
        use nostr_release_kit::ApkStatus;
        let ev: Event = serde_json::from_str(EVENT).unwrap();
        let mut cfg = update_config(None);
        cfg.release_pubkey = Some(PublicKey::from_hex(TEST_PUBKEY).unwrap());
        let mut u = Updater::new(cfg, Box::new(AppFetcher));
        u.set_installed(500);
        u.apply_events(&[ev], 5);
        u.demo_install_state(ApkStatus::NeedsPermission);
        let v = u.view(6);
        assert_eq!(v.status, "Allow installs from Cluster Flock in the settings page that opened, then tap Update again");
        assert_eq!(v.installed, crate::version_label());
    }

    #[test]
    fn a_staged_offer_never_reaches_the_state_file() {
        let dir = std::env::temp_dir().join(format!("flock-release-test-{}", std::process::id()));
        let mut file_backed = updater(Some(&dir));
        file_backed.demo_offer("0.3.0", "x");
        assert!(matches!(file_backed.verdict(), Verdict::Unknown), "a file-backed updater refuses a throwaway-signed offer");
        assert!(!dir.join(STATE_FILE).exists());
        let mut shots = updater(None);
        shots.demo_offer("0.3.0", "Faster starts.");
        assert!(matches!(shots.verdict(), Verdict::Offer(_)));
    }
}
