//! Where the app keeps its files (profile and settings).
//!
//! Desktop: `--data-dir`, else `$XDG_DATA_HOME/gameflock`, else
//! `$HOME/.local/share/gameflock`. Android: the activity's internal data path
//! (set by `android_main`). Everything is one JSON file, `profile.json`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The default desktop data directory.
pub fn default_dir() -> PathBuf {
    if let Some(x) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(x).join("gameflock");
    }
    if let Some(h) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(h).join(".local/share/gameflock");
    }
    PathBuf::from(".gameflock")
}

/// Graphics quality (Settings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Low,
    #[default]
    Balanced,
    High,
}

/// The player's name and settings. Stored as `profile.json`; every field has a
/// default, so an older file (or a missing one) still loads. Add new settings
/// here with `#[serde(default)]` semantics (the struct is `#[serde(default)]`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Display name, 12 characters max.
    pub name: String,
    pub music_volume: f32,
    pub effects_volume: f32,
    pub haptics: bool,
    pub quality: Quality,
    /// Mirror the thumb layout.
    pub left_handed: bool,
    /// The splash and first-run hint have been seen.
    pub seen_intro: bool,
    /// Random seed the default name comes from (0 = not drawn yet; `load` draws it).
    pub name_seed: u32,
    /// Furthest run so far, in metres.
    pub best_distance_m: u32,
    /// Most coins collected in one run.
    pub best_coins: u32,
    /// The player's colour/accessory in multiplayer (0..6); `None` = drawn from `name_seed`.
    pub colour: Option<u8>,
    /// Gentler camera shake on hits (Cluster Flock run screen).
    pub reduced_shake: bool,
    /// Every coin ever banked (all modes). Hats are bought from it.
    pub wallet: u64,
    /// The equipped hat id (`flock_render::HATS`); 255 = the colour's own hat.
    /// `load` turns 255 into the explicit id of the first-launch colour's hat.
    pub hat: u8,
    /// Hat ids the player owns. Always contains 0 ("no hat").
    pub owned_hats: Vec<u8>,
    /// The first-launch wardrobe set-up has run (colour fixed, own hat owned).
    pub wardrobe_ready: bool,
    /// The last daily block fetched, for offline replays and the Home button.
    pub daily: Option<DailyCache>,
    /// Best run on the current daily block.
    pub daily_best: Option<DailyBest>,
}

/// The daily block the app last learned of (see `docs/SPEC.md` section 14).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DailyCache {
    /// UTC day number.
    pub day: u64,
    /// The block height, the daily's public number.
    pub height: u32,
    pub hash_hex: String,
    pub seed: u64,
    /// Unix time of the fetch (the phone clock; only used to space refetches).
    pub fetched_unix: u64,
}

/// The best run on one daily block.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DailyBest {
    pub height: u32,
    pub metres: u32,
    pub coins: u32,
    pub attempts: u32,
}

/// The hat id of colour `c`'s own look (hat ids follow `PlayerStyle::of`: crown,
/// cap, viking helmet, mohawk, star headband, scarf are ids 1..=6).
pub fn default_hat_of(colour: u8) -> u8 {
    colour % 6 + 1
}

/// A hat purchase that did not go through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuyError {
    /// This many more coins are needed.
    Short(u64),
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            name: PLAIN_NAME.into(),
            music_volume: 0.6,
            effects_volume: 0.9,
            haptics: true,
            quality: Quality::Balanced,
            left_handed: false,
            seen_intro: false,
            name_seed: 0,
            best_distance_m: 0,
            best_coins: 0,
            colour: None,
            reduced_shake: false,
            wallet: 0,
            hat: flock_render::HAT_DEFAULT,
            owned_hats: vec![0],
            wardrobe_ready: false,
            daily: None,
            daily_best: None,
        }
    }
}

const FILE: &str = "profile.json";

/// The default relay list (the shared library's), as the account code uses it.
pub fn default_relays() -> Vec<String> {
    decented_net::DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect()
}

/// Longest display name.
pub const NAME_MAX: usize = 12;

/// The name a profile has until the player picks one.
pub const PLAIN_NAME: &str = "Player";

const NAME_ADJ: [&str; 16] = ["Fluffy", "Swift", "Zippy", "Lucky", "Sunny", "Bold", "Wild", "Jolly", "Brave", "Rapid", "Downy", "Fast", "Zany", "Tiny", "Mega", "Epic"];
const NAME_NOUN: [&str; 14] = ["Flap", "Beak", "Wing", "Egg", "Nest", "Hop", "Quill", "Soar", "Dart", "Peck", "Coop", "Roost", "Flock", "Glide"];

/// A fun default name like "FluffyFlap 27" from `seed`: always the same for the
/// same seed, and at most 12 characters (the name limit, also the online one).
pub fn default_name(seed: u32) -> String {
    // A small mix so neighbouring seeds do not give neighbouring words.
    let h = seed.wrapping_mul(0x9E37_79B1) ^ (seed >> 15);
    let adj = NAME_ADJ[(h % NAME_ADJ.len() as u32) as usize];
    let room = NAME_MAX - adj.len() - 3; // a space and two digits
    let fits: Vec<&str> = NAME_NOUN.iter().copied().filter(|n| n.len() <= room).collect();
    let noun = fits[((h >> 8) % fits.len() as u32) as usize];
    format!("{adj}{noun} {:02}", (h >> 16) % 100)
}

/// A random non-zero seed (the clock and the process id; this is not security).
fn random_seed() -> u32 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0x1234_5678);
    let m = n ^ (n >> 32) ^ ((std::process::id() as u64) << 11);
    (m as u32).max(1)
}

/// A display name cleaned for other players to see: no control characters,
/// trimmed, at most [`NAME_MAX`] characters, `fallback` when nothing is left.
pub fn sanitize_name(name: &str, fallback: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control()).take(NAME_MAX).collect();
    let n = n.trim();
    if n.is_empty() {
        fallback.to_string()
    } else {
        n.to_string()
    }
}

impl Profile {
    /// Load `profile.json` from `dir`; defaults if it is missing or unreadable
    /// (a corrupt file is kept as `profile.json.bad`).
    pub fn load(dir: &Path) -> Profile {
        let path = dir.join(FILE);
        let Ok(text) = std::fs::read_to_string(&path) else { return Profile::default().with_default_name() };
        match serde_json::from_str::<Profile>(&text) {
            Ok(mut p) => {
                p.sanitize();
                p.with_default_name()
            }
            Err(e) => {
                log::warn!("profile: {} is unreadable ({e}); using defaults", path.display());
                let _ = std::fs::rename(&path, dir.join("profile.json.bad"));
                Profile::default().with_default_name()
            }
        }
    }

    /// First run, or a profile that still says "Player": draw the name seed
    /// (once) and take the generated name. A name the player chose later is
    /// never touched, even if it is "Player".
    fn with_default_name(mut self) -> Profile {
        if self.name_seed == 0 {
            self.name_seed = random_seed();
            if self.name == PLAIN_NAME {
                self.name = default_name(self.name_seed);
            }
        }
        self.ensure_wardrobe();
        self
    }

    /// First launch with the wardrobe: pin the colour to what Home showed so far
    /// (nobody's bird changes colour on update), own "no hat" and the hat that
    /// colour wore, and wear it. Runs once.
    pub fn ensure_wardrobe(&mut self) {
        if !self.owned_hats.contains(&0) {
            self.owned_hats.insert(0, 0);
        }
        if !self.wardrobe_ready && self.name_seed != 0 {
            // Haptics were switched off in every build before 0.2.0, so a stored `false`
            // was never the player's choice: start them on once; Settings turns them off.
            self.haptics = true;
            let c = self.preferred_colour();
            self.colour = Some(c);
            let own = default_hat_of(c);
            if !self.owned_hats.contains(&own) {
                self.owned_hats.push(own);
            }
            if self.hat == flock_render::HAT_DEFAULT {
                self.hat = own;
            }
            self.wardrobe_ready = true;
        }
    }

    /// The local player's look: (colour 0..6, hat id).
    pub fn look(&self) -> (u8, u8) {
        (self.preferred_colour(), self.hat)
    }

    pub fn owns_hat(&self, id: u8) -> bool {
        id == 0 || self.owned_hats.contains(&id)
    }

    /// Add `coins` to the wallet (saturating).
    pub fn bank(&mut self, coins: u32) {
        self.wallet = self.wallet.saturating_add(coins as u64);
    }

    /// Buy hat `id` for `price` coins and wear it. Nothing changes on failure.
    pub fn buy_hat(&mut self, id: u8, price: u32) -> Result<(), BuyError> {
        if self.owns_hat(id) {
            self.hat = id;
            return Ok(());
        }
        if self.wallet < price as u64 {
            return Err(BuyError::Short(price as u64 - self.wallet));
        }
        self.wallet -= price as u64;
        self.owned_hats.push(id);
        self.hat = id;
        Ok(())
    }

    /// Best daily run on `height`, if any.
    pub fn daily_best_on(&self, height: u32) -> Option<&DailyBest> {
        self.daily_best.as_ref().filter(|b| b.height == height)
    }

    /// The colour asked for in multiplayer lobbies (0..6): the saved choice, else one drawn from the name seed.
    pub fn preferred_colour(&self) -> u8 {
        self.colour.unwrap_or((self.name_seed % 6) as u8) % 6
    }

    /// What an emptied name field becomes.
    pub fn fallback_name(&self) -> String {
        if self.name_seed == 0 {
            PLAIN_NAME.into()
        } else {
            default_name(self.name_seed)
        }
    }

    /// Write `profile.json` (to a temp file first, then renamed over it).
    pub fn save(&self, dir: &Path) {
        if let Err(e) = self.try_save(dir) {
            log::warn!("profile: could not save: {e}");
        }
    }

    fn try_save(&self, dir: &Path) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = dir.join("profile.json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| e.to_string())
    }

    /// Clamp loaded values into range.
    pub fn sanitize(&mut self) {
        self.name = self.name.chars().filter(|c| !c.is_control()).take(NAME_MAX).collect();
        if self.name.trim().is_empty() {
            self.name = self.fallback_name();
        }
        self.music_volume = if self.music_volume.is_finite() { self.music_volume.clamp(0.0, 1.0) } else { 0.6 };
        self.effects_volume = if self.effects_volume.is_finite() { self.effects_volume.clamp(0.0, 1.0) } else { 0.9 };
        self.owned_hats.sort_unstable();
        self.owned_hats.dedup();
        self.ensure_wardrobe();
        if self.hat != flock_render::HAT_DEFAULT && !self.owns_hat(self.hat) {
            self.hat = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gameflock-profile-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip_and_partial_files() {
        let dir = temp("rt");
        // First run: a generated name and a drawn seed, nothing else different.
        let first = Profile::load(&dir);
        assert_ne!(first.name_seed, 0);
        assert_eq!(first.name, default_name(first.name_seed));
        assert!(first.wardrobe_ready && first.colour.is_some() && first.owned_hats.len() == 2);
        let undone = Profile { name: "Player".into(), name_seed: 0, colour: None, hat: flock_render::HAT_DEFAULT, owned_hats: vec![0], wardrobe_ready: false, ..first };
        assert_eq!(undone, Profile::default());
        let mut p = Profile { name: "Ada".into(), left_handed: true, name_seed: 7, best_distance_m: 1234, best_coins: 56, ..Profile::default() };
        p.ensure_wardrobe();
        p.save(&dir);
        assert_eq!(Profile::load(&dir), p);
        // An older file with only some fields still loads (the new bests default to 0).
        std::fs::write(dir.join(FILE), r#"{"name":"Bo","music_volume":7.0}"#).unwrap();
        let q = Profile::load(&dir);
        assert_eq!(q.name, "Bo");
        assert_eq!(q.music_volume, 1.0);
        assert_eq!((q.best_distance_m, q.best_coins), (0, 0));
        // Garbage falls back to defaults and is kept aside.
        std::fs::write(dir.join(FILE), "not json").unwrap();
        assert!(!Profile::load(&dir).seen_intro);
        assert!(dir.join("profile.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_old_profile_loads_unchanged_and_keeps_its_bird() {
        let dir = temp("wardrobe");
        std::fs::create_dir_all(&dir).unwrap();
        // A 0.1/0.2 file: none of the wardrobe, wallet or daily fields.
        std::fs::write(
            dir.join(FILE),
            r#"{"name":"Ada","music_volume":0.3,"haptics":true,"left_handed":true,"seen_intro":true,"name_seed":10,"best_distance_m":900,"best_coins":31,"colour":null,"reduced_shake":true}"#,
        )
        .unwrap();
        let p = Profile::load(&dir);
        assert_eq!((p.name.as_str(), p.music_volume, p.haptics, p.left_handed, p.seen_intro), ("Ada", 0.3, true, true, true));
        assert_eq!((p.name_seed, p.best_distance_m, p.best_coins, p.reduced_shake), (10, 900, 31, true));
        // Home showed name_seed % 6 = 4 before; it still does, now pinned.
        assert_eq!(p.preferred_colour(), 4);
        assert_eq!(p.colour, Some(4));
        assert_eq!((p.wallet, p.daily.clone(), p.daily_best.clone()), (0, None, None));
        // It owns no hat and the star headband its colour wore.
        assert_eq!(p.owned_hats, vec![0, 5]);
        assert_eq!(p.hat, 5);
        // And the wardrobe set-up does not run twice.
        let mut q = p.clone();
        q.colour = Some(1);
        q.ensure_wardrobe();
        assert_eq!(q.colour, Some(1));
        assert_eq!(q.owned_hats, vec![0, 5]);
        // A file with a saved colour keeps it.
        std::fs::write(dir.join(FILE), r#"{"name":"Bo","name_seed":10,"colour":2}"#).unwrap();
        let p = Profile::load(&dir);
        assert_eq!((p.colour, p.hat, p.owned_hats.clone()), (Some(2), 3, vec![0, 3]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_hats_match_the_renderer() {
        for c in 0..6 {
            assert_eq!(flock_render::hat_accessory(default_hat_of(c)), Some(flock_render::PlayerStyle::of(c).accessory), "colour {c}");
        }
    }

    #[test]
    fn buying_hats_spends_the_wallet_once() {
        let mut p = Profile { wallet: 200, ..Profile::default() };
        assert_eq!(p.buy_hat(2, 300), Err(BuyError::Short(100)));
        assert_eq!((p.wallet, p.hat), (200, flock_render::HAT_DEFAULT));
        p.bank(150);
        assert_eq!(p.wallet, 350);
        assert_eq!(p.buy_hat(2, 300), Ok(()));
        assert_eq!((p.wallet, p.hat, p.owned_hats.clone()), (50, 2, vec![0, 2]));
        // Owning it already: wearing it again is free.
        assert_eq!(p.buy_hat(2, 300), Ok(()));
        assert_eq!(p.wallet, 50);
        assert_eq!(p.buy_hat(0, 999), Ok(()));
        assert_eq!(p.hat, 0);
        p.wallet = u64::MAX;
        p.bank(5);
        assert_eq!(p.wallet, u64::MAX);
    }

    #[test]
    fn unknown_old_fields_are_ignored() {
        // Unknown keys (an old shared profile) must not break loading.
        let dir = temp("old");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), r#"{"name":"Ada","steer_mode":"tilt","tilt_sensitivity":2.0}"#).unwrap();
        assert_eq!(Profile::load(&dir).name, "Ada");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_names_are_distinct_short_and_stable() {
        let mut seen = std::collections::HashSet::new();
        for seed in 1..200u32 {
            let n = default_name(seed);
            assert!(n.chars().count() <= NAME_MAX, "{n}");
            assert_eq!(n, default_name(seed));
            seen.insert(n);
        }
        assert!(seen.len() > 100);
    }

    #[test]
    fn a_player_name_survives_but_an_old_plain_one_gets_a_name_once() {
        let dir = temp("name");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), r#"{"name":"Player"}"#).unwrap();
        let p = Profile::load(&dir);
        assert_ne!(p.name, "Player");
        std::fs::write(dir.join(FILE), r#"{"name":"Player","name_seed":9}"#).unwrap();
        assert_eq!(Profile::load(&dir).name, "Player", "a chosen name is never touched");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hand_edited_values_are_pulled_into_range() {
        let mut p = Profile { name: "  ".into(), music_volume: f32::NAN, effects_volume: -3.0, name_seed: 5, ..Profile::default() };
        p.sanitize();
        assert_eq!(p.name, default_name(5));
        assert_eq!(p.music_volume, 0.6);
        assert_eq!(p.effects_volume, 0.0);
    }

    #[test]
    fn display_names_are_cleaned() {
        assert_eq!(sanitize_name("  Ada\n ", "Flocker"), "Ada");
        assert_eq!(sanitize_name("\u{7}", "Flocker"), "Flocker");
        assert_eq!(sanitize_name("abcdefghijklmnopqrstuvwxyz", "x").chars().count(), NAME_MAX);
    }
}
