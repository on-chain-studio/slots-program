//! Adapter onto `slots-engine`, the crate the client also runs as wasm.
//!
//! There is no field copying here: the engine reads the machine's stored bytes directly, so this
//! is only the cast plus the error mapping. That is deliberate — a hand-written conversion is
//! exactly where the chain and the client would drift apart.

use crate::chain::*;

use crate::state::config::MachineConfig;

pub use slots_engine::{Grid, Stops, Winnings};

fn shared(machine: &MachineConfig) -> Result<slots_engine::MachineConfig, ProgramError> {
    slots_engine::parse(bytemuck::bytes_of(machine)).ok_or(ProgramError::InvalidAccountData)
}

/// Where the reels come to rest, given a seed and whatever is being held.
pub fn spin(
    machine: &MachineConfig,
    seed: &[u8; 32],
    hold: u8,
    prev: &Stops,
) -> Result<Stops, ProgramError> {
    slots_engine::spin(&shared(machine)?, seed, hold, prev)
        .map_err(|_| ProgramError::InvalidAccountData)
}

/// What a resting grid pays.
pub fn value(machine: &MachineConfig, stops: &Stops) -> Result<Winnings, ProgramError> {
    slots_engine::value(&shared(machine)?, stops).map_err(|_| ProgramError::InvalidAccountData)
}

/// One rung of the ladder: true doubles, false takes it all.
pub fn gamble(machine: &MachineConfig, seed: &[u8; 32]) -> Result<bool, ProgramError> {
    slots_engine::gamble(&shared(machine)?, seed).map_err(|_| ProgramError::InvalidAccountData)
}
