//! More games (spec 20.3): the catalog of other games built into the app, as
//! data. There is no remote list. Each entry pins the game's package, its
//! release channel, the key that signs its release events and the certificate
//! its APK is signed with.
//!
//! The rules (judging an entry's release event by THAT entry's key and channel,
//! the status a card shows, the package and certificate check on the downloaded
//! file) are nostr-release-kit's `Catalog`; Cluster Flock keeps only the entries
//! and their icons here. See `crate::release` for the `Catalog` built from them.

use nostr_release_kit::CatalogEntry;

/// Cluster Flock's catalog: the three other games. It omits Cluster Flock itself.
/// The pins are copied from those games' own catalogs; keep the manifest
/// `<queries>` in step. Order: as shown (and as [`ICONS`]).
pub const CATALOG_ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "Twang Thang",
        blurb: "A neon energy-band party game.",
        package: "org.decented.twangthang",
        channel: "twangthang-android-apk",
        release_pubkey: Some("2cc65a54774d9c1d6fde7e98255dbe1064940facb10dbc3e49f63300a27178c7"),
        cert: Some("ad02f430148557fb6501c97b16a9ef60681d12d533f7579035e6d62a83dec640"),
    },
    CatalogEntry {
        name: "Stampede",
        blurb: "Laser-eyed ostrich racing, arcade style.",
        package: "org.decented.stampede",
        channel: "stampede-android-apk",
        // Stampede's own release key (npub1ka2v5p8ansyaz7uwzgu5aan4g3r4qfdug34vpe2jvavgcgcj3z5qqa984j)
        // and its APK signing certificate, pinned 2026-10-13.
        release_pubkey: Some("b754ca04fd9c09d17b8e12394ef67544475025bc446ac0e55267588c231288a8"),
        cert: Some("eacd24505a50630e2a115226fe592518bc16cc2421cfc019372b58baeaf08aa9"),
    },
    CatalogEntry {
        name: "Axe'n'Stax",
        blurb: "Build, mine and explore a world you own.",
        package: "com.axenstax.game",
        channel: "axenstax-android-apk",
        // Axe'n'Stax's release key and its permanent release keystore's certificate, pinned
        // 2026-11-13 from its 0.2.36 event (76d97989…) and APK, both checked.
        release_pubkey: Some("ca43e7218d03a50e20b07f54fad58a7cf4ef9ac1a883e3a0104255a46600e502"),
        cert: Some("d27f95670c4c69720dc4b679b82cc54b0ad62b0e34b68a917a1e72ae88a1c642"),
    },
];

/// The entries' icons (PNG, bundled in the app), indexed like [`CATALOG_ENTRIES`].
pub const ICONS: [&[u8]; 3] = [
    include_bytes!("../assets/games/twangthang.png"),
    include_bytes!("../assets/games/stampede.png"),
    include_bytes!("../assets/games/axenstax.png"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::OWN_PACKAGE;

    #[test]
    fn the_catalog_pins_are_well_formed() {
        assert_eq!(CATALOG_ENTRIES.len(), ICONS.len(), "one icon per entry");
        for (e, icon) in CATALOG_ENTRIES.iter().zip(ICONS) {
            assert!(e.ready(), "{} should be fetchable", e.name);
            assert!(!e.name.is_empty() && !e.blurb.is_empty() && e.blurb.len() < 60 && !icon.is_empty());
            assert_ne!(e.package, OWN_PACKAGE, "the catalog omits this app itself");
        }
        assert_eq!(CATALOG_ENTRIES[0].package, "org.decented.twangthang");
        assert_eq!(CATALOG_ENTRIES[0].channel, "twangthang-android-apk");
        assert_eq!(CATALOG_ENTRIES[1].package, "org.decented.stampede");
        assert_eq!(CATALOG_ENTRIES[1].channel, "stampede-android-apk");
        assert_eq!(CATALOG_ENTRIES[2].package, "com.axenstax.game");
        assert_eq!(CATALOG_ENTRIES[2].channel, "axenstax-android-apk");
        // Pinned keys and certificates, exactly as shipped.
        assert_eq!(CATALOG_ENTRIES[0].release_pubkey, Some("2cc65a54774d9c1d6fde7e98255dbe1064940facb10dbc3e49f63300a27178c7"));
        assert_eq!(CATALOG_ENTRIES[0].cert, Some("ad02f430148557fb6501c97b16a9ef60681d12d533f7579035e6d62a83dec640"));
        assert_eq!(CATALOG_ENTRIES[1].release_pubkey, Some("b754ca04fd9c09d17b8e12394ef67544475025bc446ac0e55267588c231288a8"));
        assert_eq!(CATALOG_ENTRIES[1].cert, Some("eacd24505a50630e2a115226fe592518bc16cc2421cfc019372b58baeaf08aa9"));
        assert_eq!(CATALOG_ENTRIES[2].release_pubkey, Some("ca43e7218d03a50e20b07f54fad58a7cf4ef9ac1a883e3a0104255a46600e502"));
        assert_eq!(CATALOG_ENTRIES[2].cert, Some("d27f95670c4c69720dc4b679b82cc54b0ad62b0e34b68a917a1e72ae88a1c642"));
    }

    /// Axe'n'Stax's real 0.2.36 release event, as relay.ditto.pub and relay.primal.net serve it
    /// (fetched 2026-11-13). Its APK was downloaded and read with aapt2 and apksigner: package
    /// com.axenstax.game, versionCode 2036, one signer, certificate d27f9567…c642.
    const AXENSTAX_0_2_36: &str = r#"{"content":"Softer thunder and a /weather command, More games in the lobby, safer respawns, flat item sprites, bigger chat text with Tab completion.","created_at":1794568039,"id":"76d97989a824d0ebed20e799f644f236ee675daded61c3222405f265a414577f","kind":30063,"pubkey":"ca43e7218d03a50e20b07f54fad58a7cf4ef9ac1a883e3a0104255a46600e502","sig":"ecc4e80dfae5ede7cae071f03a2a06b6c2f4b7b845b4abef73f38e3c74be2cd1a5674efc32332581fe7e6abc4d2a68d8cd2ce93eaa974047b833da344752c210","tags":[["d","axenstax-android-apk"],["version","0.2.36"],["version_code","2036"],["x","dd493d40694deed00292b292f371b5178720447227825da75e28be1ee3e755a6"],["size","20317642"],["cert","d27f95670c4c69720dc4b679b82cc54b0ad62b0e34b68a917a1e72ae88a1c642"],["url","https://nostr.download/dd493d40694deed00292b292f371b5178720447227825da75e28be1ee3e755a6.apk"]]}"#;

    #[test]
    fn axenstaxs_published_release_is_offered_through_its_pins() {
        use nostr_release_kit::{judge_archive, release_for, status_of, EntryStatus, Fetch};
        let ev: nostr::Event = serde_json::from_str(AXENSTAX_0_2_36).unwrap();
        let axe = &CATALOG_ENTRIES[2];
        let r = release_for(axe, std::slice::from_ref(&ev));
        assert_eq!(status_of(axe, &Fetch::Answered, &r, None), EntryStatus::Get { version: "0.2.36".into(), size: 20_317_642 });
        assert_eq!(status_of(axe, &Fetch::Answered, &r, Some(2036)), EntryStatus::Open);
        let release = r.unwrap().unwrap();
        assert_eq!(release.version_code, 2036);
        // The downloaded file as read on 2026-11-13 passes the strict (catalog) archive check.
        let got_cert = "d27f95670c4c69720dc4b679b82cc54b0ad62b0e34b68a917a1e72ae88a1c642";
        assert!(judge_archive("com.axenstax.game", got_cert, axe.package, axe.cert.unwrap(), &release.cert, true, false));
        // The other entries' keys and channels don't take it.
        for other in &CATALOG_ENTRIES[..2] {
            assert!(matches!(release_for(other, std::slice::from_ref(&ev)), Ok(None)), "{}", other.name);
        }
        // A copy with its version changed no longer matches its signature, and is not offered.
        let forged: nostr::Event = serde_json::from_str(&AXENSTAX_0_2_36.replace("\"2036\"", "\"9036\"")).unwrap();
        assert!(matches!(release_for(axe, &[forged]), Ok(None)));
    }

    #[test]
    fn the_manifest_queries_name_every_catalog_package() {
        let manifest = include_str!("../android/AndroidManifest.xml");
        for e in CATALOG_ENTRIES {
            assert!(manifest.contains(&format!("<package android:name=\"{}\" />", e.package)), "{} missing from <queries>", e.package);
        }
    }
}
