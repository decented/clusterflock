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

/// Cluster Flock's catalog: the two other games. It omits Cluster Flock itself.
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
];

/// The entries' icons (PNG, bundled in the app), indexed like [`CATALOG_ENTRIES`].
pub const ICONS: [&[u8]; 2] = [include_bytes!("../assets/games/twangthang.png"), include_bytes!("../assets/games/stampede.png")];

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
        // Pinned keys and certificates, exactly as shipped.
        assert_eq!(CATALOG_ENTRIES[0].release_pubkey, Some("2cc65a54774d9c1d6fde7e98255dbe1064940facb10dbc3e49f63300a27178c7"));
        assert_eq!(CATALOG_ENTRIES[0].cert, Some("ad02f430148557fb6501c97b16a9ef60681d12d533f7579035e6d62a83dec640"));
        assert_eq!(CATALOG_ENTRIES[1].release_pubkey, Some("b754ca04fd9c09d17b8e12394ef67544475025bc446ac0e55267588c231288a8"));
        assert_eq!(CATALOG_ENTRIES[1].cert, Some("eacd24505a50630e2a115226fe592518bc16cc2421cfc019372b58baeaf08aa9"));
    }

    #[test]
    fn the_manifest_queries_name_every_catalog_package() {
        let manifest = include_str!("../android/AndroidManifest.xml");
        for e in CATALOG_ENTRIES {
            assert!(manifest.contains(&format!("<package android:name=\"{}\" />", e.package)), "{} missing from <queries>", e.package);
        }
    }
}
