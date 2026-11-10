//! Events to effects: sounds, particle bursts and HUD signals. Positions are
//! render space (x already mirrored by the caller).

use flock_audio::{coin_pitch, LoopId, Sfx};
use flock_core::{Event, PlayerId, PowerKind, Tool};
use flock_render::{Burst, BurstKind, PlayerStyle, PowerShape};
use glam::Vec3;

use crate::sound;

/// What the HUD and the Run screen should react to this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Signal {
    Go,
    LocalFeatherLost,
    LocalCoin,
    LocalCrash,
    LocalBump,
    LocalBecameGhost,
    LocalLanded,
    LocalProtectionEnd,
    /// The free hit of a protection episode: the bird carried on through an obstacle.
    LocalSwagger,
    /// A jump was refused in mud.
    LocalMudStuck,
    /// The local player collected a power-up.
    LocalPower(PowerKind),
    /// The local player's shield absorbed a crash.
    LocalShieldBroken,
    /// An object the local player placed slammed down.
    LocalSlam,
    /// A countdown beep (3, 2, 1).
    Beep,
    /// A ghost (anyone) dropped something.
    Placed { by: PlayerId, tool: Tool },
}

/// Sound state that spans frames.
#[derive(Default)]
pub struct Fx {
    coin_chain: u32,
    coin_idle: f32,
    last_beep: i32,
}

/// Where things are, for effect placement.
pub trait Where {
    /// Render-space position of a player's body (feet).
    fn at(&self, id: PlayerId) -> Vec3;
    /// Palette colour of a player.
    fn colour(&self, id: PlayerId) -> u8;
    /// Does the player hold the Doubler right now?
    fn doubler(&self, id: PlayerId) -> bool;
}

/// The renderer's pickup shape for a sim power-up kind.
pub fn shape_of(k: PowerKind) -> PowerShape {
    match k {
        PowerKind::Magnet => PowerShape::Magnet,
        PowerKind::Doubler => PowerShape::Doubler,
        PowerKind::Shield => PowerShape::Shield,
    }
}

fn play(s: Sfx, gain: f32, pitch: f32) {
    sound::with(|a| a.play(s, gain, pitch));
}

fn squawk_pitch(colour: u8) -> f32 {
    0.78 + 0.16 * (colour % 6) as f32
}

impl Fx {
    pub fn new() -> Fx {
        Fx { last_beep: 99, ..Fx::default() }
    }

    /// Process one event: appends bursts and signals, plays sounds.
    pub fn event(&mut self, e: &Event, local: PlayerId, w: &dyn Where, bursts: &mut Vec<Burst>, signals: &mut Vec<Signal>, shake: &mut f32) {
        let g = |p: PlayerId| if p == local { 1.0 } else { 0.35 };
        let rgb = |p: PlayerId| PlayerStyle::of(w.colour(p)).body;
        match *e {
            Event::Go => {
                signals.push(Signal::Go);
                play(Sfx::CountdownGo, 1.0, 1.0);
            }
            Event::Jumped { player } => play(Sfx::Jump, g(player) * 0.8, 1.0 + 0.04 * w.colour(player) as f32),
            Event::Landed { player } => {
                let p = w.at(player);
                bursts.push(Burst::new(BurstKind::LandingDust, p, if player == local { 24 } else { 12 }));
                play(Sfx::Land, g(player) * 0.7, 1.0);
                if player == local {
                    signals.push(Signal::LocalLanded);
                }
            }
            Event::Hop { player, .. } => play(Sfx::Flap, g(player) * 0.8, 1.0),
            Event::Collapsed { player } => {
                bursts.push(Burst::new(BurstKind::LandingDust, w.at(player), 10));
            }
            Event::Tumble { player } => play(Sfx::Squawk, g(player) * 0.7, squawk_pitch(w.colour(player))),
            Event::Bump { a, b, speed } => {
                let mid = (w.at(a) + w.at(b)) * 0.5 + Vec3::Y * 0.9;
                bursts.push(Burst::new(BurstKind::HitPuff, mid, 10));
                let loud = (0.5 + speed * 0.08).min(1.0);
                let mine = a == local || b == local;
                play(Sfx::Bump, loud * if mine { 1.0 } else { 0.4 }, 0.9 + (speed * 0.03).min(0.4));
                if mine {
                    signals.push(Signal::LocalBump);
                    *shake += 0.25;
                }
            }
            Event::Crash { player, feather_lost, .. } => {
                let p = w.at(player) + Vec3::Y * 0.9;
                if feather_lost {
                    bursts.push(Burst::new(BurstKind::Feathers, p, 18).with_color(rgb(player)));
                } else {
                    bursts.push(Burst::new(BurstKind::HitPuff, p, 10));
                    bursts.push(Burst::new(BurstKind::Feathers, p, 6).with_color(rgb(player)));
                }
                play(Sfx::Crash, g(player), 1.0);
                play(Sfx::Squawk, g(player) * 0.8, squawk_pitch(w.colour(player)));
                if player == local {
                    signals.push(Signal::LocalCrash);
                    if feather_lost {
                        signals.push(Signal::LocalFeatherLost);
                    }
                    *shake += if feather_lost { 0.8 } else { 0.4 };
                }
            }
            Event::MudEnter { player } => {
                bursts.push(Burst::new(BurstKind::MudSplat, w.at(player) + Vec3::Y * 0.1, 20));
                play(Sfx::Mud, g(player), 1.0);
            }
            Event::CoinTaken { by, .. } => {
                bursts.push(Burst::new(BurstKind::CoinSparkle, w.at(by) + Vec3::Y * 0.9, if w.doubler(by) { 20 } else { 10 }));
                if by == local {
                    play(Sfx::Coin, 0.85, coin_pitch(self.coin_chain));
                    self.coin_chain += 1;
                    self.coin_idle = 0.0;
                    signals.push(Signal::LocalCoin);
                } else {
                    play(Sfx::Coin, 0.2, 1.0);
                }
            }
            Event::BecameGhost { player } => {
                bursts.push(Burst::new(BurstKind::GhostPoof, w.at(player) + Vec3::Y * 0.9, 24).with_color(rgb(player)));
                play(Sfx::GhostWhoosh, g(player), 1.0);
                if player == local {
                    signals.push(Signal::LocalBecameGhost);
                }
            }
            Event::ProtectionEnd { player } if player == local => {
                play(Sfx::ProtectEnd, 0.7, 1.0);
                signals.push(Signal::LocalProtectionEnd);
            }
            Event::Placed { by, tool, .. } => {
                signals.push(Signal::Placed { by, tool });
            }
            Event::Materialised { .. } => play(Sfx::PlaceThud, 0.7, 1.0),
            Event::Swagger { player, .. } => {
                // the free hit: a star puff round the head and a soft bonk, no crash
                bursts.push(Burst::new(BurstKind::Sparkle, w.at(player) + Vec3::Y * 2.5, 8).with_color([1.0, 0.9, 0.35]));
                play(Sfx::Bump, g(player) * 0.55, 0.7);
                if player == local {
                    signals.push(Signal::LocalSwagger);
                    *shake += 0.3;
                }
            }
            Event::MudStuck { player } => {
                bursts.push(Burst::new(BurstKind::MudSplat, w.at(player) + Vec3::Y * 0.1, 8));
                play(Sfx::Mud, g(player) * 0.7, 1.35);
                if player == local {
                    signals.push(Signal::LocalMudStuck);
                }
            }
            Event::PowerTaken { by, kind, .. } => {
                let shape = shape_of(kind);
                bursts.push(Burst::new(BurstKind::Sparkle, w.at(by) + Vec3::Y * 1.2, 30).with_color(shape.colour()));
                let (sfx, pitch) = match kind {
                    PowerKind::Magnet => (Sfx::ItemGet, 1.0),
                    PowerKind::Doubler => (Sfx::ItemGet, 1.35),
                    PowerKind::Shield => (Sfx::ShieldUp, 1.0),
                };
                play(sfx, g(by), pitch);
                if by == local {
                    signals.push(Signal::LocalPower(kind));
                }
            }
            Event::ShieldBroken { player } => {
                bursts.push(Burst::new(BurstKind::Sparkle, w.at(player) + Vec3::Y * 1.3, 32).with_color(PowerShape::Shield.colour()));
                bursts.push(Burst::new(BurstKind::HitPuff, w.at(player) + Vec3::Y * 1.0, 8));
                play(Sfx::ShieldBlock, g(player), 1.0);
                if player == local {
                    signals.push(Signal::LocalShieldBroken);
                    *shake += 0.35;
                }
            }
            _ => {}
        }
    }

    /// 3-2-1 beeps from the countdown clock (seconds to GO). Call every frame.
    pub fn countdown(&mut self, left: f32, signals: &mut Vec<Signal>) {
        if left <= 0.0 {
            self.last_beep = 99;
            return;
        }
        let n = left.ceil() as i32;
        if n != self.last_beep && n <= 3 {
            self.last_beep = n;
            play(Sfx::CountdownBeep, 0.8, 1.0);
            signals.push(Signal::Beep);
        }
    }

    /// Footsteps and wind from the local runner. `speed` m/s, `grounded`.
    pub fn loops(&mut self, dt: f32, speed: f32, grounded: bool, running: bool, top: f32) {
        self.coin_idle += dt;
        if self.coin_idle > 1.0 {
            self.coin_chain = 0;
        }
        let s = (speed / top.max(1.0)).clamp(0.0, 1.2);
        sound::with(|a| {
            a.set_loop(LoopId::Footsteps, if grounded && running { (s * 3.0).min(0.7) } else { 0.0 }, 0.6 + 1.5 * s);
            a.set_loop(LoopId::Wind, if running { s * s * 0.5 } else { 0.0 }, 0.8 + 0.6 * s);
        });
    }

    /// Silence the loops (pause, leaving).
    pub fn quiet() {
        sound::with(|a| a.stop_loops());
    }
}
