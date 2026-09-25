//! This game's half of the shared `Custom(n)` space. `casino_core::CoreError` owns 1–3, 5–7 and
//! 9–11; 4 and 8 are left to each game to name, and 12 up are its own.

use casino_core::chain::*;

#[derive(Debug)]
#[repr(u32)]
pub enum GameError {
    /// No such machine on the shelf, or one the engine refuses to play.
    InvalidMachine     = 4,
    /// The randomness for this round has not landed yet.
    NotRolled          = 8,
    /// Every round this machine offers has been played: collect, don't hold or climb again.
    RoundsExhausted    = 12,
    /// A hold names a reel this machine does not have.
    InvalidHold        = 13,
}

impl From<GameError> for ProgramError {
    fn from(e: GameError) -> Self {
        ProgramError::Custom(e as u32)
    }
}
