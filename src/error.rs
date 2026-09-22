use crate::chain::*;

#[derive(Debug)]
#[repr(u32)]
pub enum GameError {
    InvalidPDA         = 1,
    Unauthorized       = 2,
    AlreadyInitialized = 3,
    InvalidMachine     = 4,
    WrongStatus        = 5,
    InsufficientFunds  = 6,
    InvalidMint        = 7,
    NotRolled          = 8,
    NothingToCollect   = 9,
    NotPaid            = 10,
    ShelfFull          = 11,
    RoundsExhausted    = 12,
    InvalidHold        = 13,
}

impl From<GameError> for ProgramError {
    fn from(e: GameError) -> Self {
        ProgramError::Custom(e as u32)
    }
}
