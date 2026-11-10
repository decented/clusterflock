//! Networked input framing: each tick a client sends `(seq, cmd)`; a command
//! is applied when `seq` differs from the last applied one, so repeated
//! frames (redundancy against loss) are harmless.

use serde::{Deserialize, Serialize};

use crate::{Command, PlayerId, MAX_PLAYERS};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InputFrame {
    /// Incremented (wrapping) by the client for every new command.
    pub seq: u8,
    pub cmd: Option<Command>,
}

const CMDS: [Command; 8] = [
    Command::Left,
    Command::Right,
    Command::Jump,
    Command::Down,
    Command::JumpLeft,
    Command::JumpRight,
    Command::DiveLeft,
    Command::DiveRight,
];

impl InputFrame {
    /// `[seq, code]`; code 0 = no command, 1..=8 = `Command` in declaration order.
    pub fn pack(self) -> [u8; 2] {
        let code = match self.cmd {
            None => 0,
            Some(c) => CMDS.iter().position(|x| *x == c).map_or(0, |i| i as u8 + 1),
        };
        [self.seq, code]
    }
    /// `None` for an unknown command code.
    pub fn unpack(b: [u8; 2]) -> Option<InputFrame> {
        let cmd = match b[1] {
            0 => None,
            n if (n as usize) <= CMDS.len() => Some(CMDS[n as usize - 1]),
            _ => return None,
        };
        Some(InputFrame { seq: b[0], cmd })
    }
}

/// Host side: turns each player's stream of frames into edge commands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CmdLatch {
    last: [Option<u8>; MAX_PLAYERS],
}

impl CmdLatch {
    pub fn new() -> Self {
        Self::default()
    }
    /// Returns the command to apply this tick, if the frame is new.
    /// Unknown player ids are ignored.
    pub fn feed(&mut self, player: PlayerId, frame: InputFrame) -> Option<Command> {
        let slot = self.last.get_mut(player.0 as usize)?;
        if *slot == Some(frame.seq) {
            return None;
        }
        *slot = Some(frame.seq);
        frame.cmd
    }
    /// Feed many frames; returns `(player, command)` pairs for `World::step`.
    pub fn feed_all(&mut self, frames: &[(PlayerId, InputFrame)]) -> Vec<(PlayerId, Command)> {
        frames.iter().filter_map(|&(p, f)| self.feed(p, f).map(|c| (p, c))).collect()
    }
    /// Forget a player's last seq (rejoin / new round).
    pub fn reset(&mut self, player: PlayerId) {
        if let Some(s) = self.last.get_mut(player.0 as usize) {
            *s = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_roundtrip_and_latch() {
        for c in CMDS.iter().map(|c| Some(*c)).chain([None]) {
            let f = InputFrame { seq: 7, cmd: c };
            assert_eq!(InputFrame::unpack(f.pack()), Some(f));
        }
        assert_eq!(InputFrame::unpack([0, 99]), None);
        let mut l = CmdLatch::new();
        let p = PlayerId(2);
        let f = InputFrame { seq: 1, cmd: Some(Command::Jump) };
        assert_eq!(l.feed(p, f), Some(Command::Jump));
        assert_eq!(l.feed(p, f), None);
        assert_eq!(l.feed(p, InputFrame { seq: 2, cmd: Some(Command::Left) }), Some(Command::Left));
        assert_eq!(l.feed(PlayerId(9), f), None);
    }
}
