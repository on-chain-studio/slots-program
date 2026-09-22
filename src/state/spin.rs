use bytemuck::{Pod, Zeroable};
use crate::chain::*;

use crate::state::config::{MachineConfig, MAX_REELS};

pub const DISCRIMINATOR: u64 = 3;
pub const VERSION:       u64 = 1;

#[repr(u64)]
#[derive(Clone, Copy, PartialEq)]
pub enum SpinStatus {
    /// Paid, and waiting for this round's randomness to be asked for.
    Bought    = 0,
    /// The VRF has been asked. A dropped callback is re-fired from here.
    Requested = 1,
    /// The seed has landed and has not been applied yet. What the client renders from.
    Rolled    = 2,
    // There is no Collected: a spin is collected when it is closed, so asking for a payout
    // leaves no state anyone could strand. Same reasoning as the card.
}

/// `["spin", user]` — one bet in flight, ephemeral. Created by the settle callback that took the
/// stake and closed by the one that pays it out, so its mere existence is the "one bet at a time"
/// mutex: no nonce to store, and no rent standing against a player who never comes back.
///
/// Carries its own `terms`, copied from the shelf at purchase, so a rebalance cannot rewrite a
/// bet someone already owns.
#[repr(C)]
#[derive(Pod, Zeroable, Clone, Copy)]
pub struct Spin {
    pub discriminator: u64,
    pub version:       u64,
    pub user:          [u8; 32],
    /// The session key that consented to the buying receipt. The vault proved it may act for
    /// this user when the stake settled, so the round decisions — hold, gamble — accept its
    /// signature without another wallet prompt. The user's own key always works too.
    pub consenter:     [u8; 32],
    pub machine_id:    u64,
    pub status:        u64,
    /// Grids resolved so far (hold), or rungs climbed (gamble). Round 0 has no stops yet.
    pub round:         u64,
    /// The hold mask committed for the *pending* seed. Committed before that seed is asked for,
    /// which is the whole reason a respin cannot be chosen with the next grid already known.
    pub hold:          u64,
    /// Lamports riding on the ladder. Zero until a base win is put at risk.
    pub pending:       u64,
    /// Where each reel came to rest. Only `reel_count` entries are meaningful.
    pub stops:         [u8; 8],
    /// This round's randomness, unapplied. Zero once it has been folded into `stops`.
    pub seed:          [u8; 32],
}

impl Spin {
    pub const SIZE: usize = size_of::<Self>();

    /// A bet placed with its terms printed after it.
    pub const WITH_TERMS: usize = Self::SIZE + size_of::<MachineConfig>();

    pub fn stops(&self) -> [u8; MAX_REELS] {
        let mut s = [0u8; MAX_REELS];
        s.copy_from_slice(&self.stops[..MAX_REELS]);
        s
    }

    pub fn set_stops(&mut self, s: &[u8; MAX_REELS]) {
        self.stops[..MAX_REELS].copy_from_slice(s);
    }

    /// Read-only, for the paths that must not write — `request_collect` reads a spin it is
    /// deliberately forbidden to mark, since requesting a payout is permissionless.
    pub fn load<'a>(account: &AccountInfo) -> Result<&'a Self, ProgramError> {
        let data = account.try_borrow()?;
        if data.len() < Self::SIZE { return Err(ProgramError::InvalidAccountData); }
        let s = bytemuck::try_from_bytes::<Self>(&data[..Self::SIZE])
            .map_err(|_| ProgramError::InvalidAccountData)
            .map(|r| unsafe { &*(r as *const Self) })?;
        if s.version != VERSION { return Err(ProgramError::InvalidAccountData); }
        Ok(s)
    }

    pub fn load_mut<'a>(account: &AccountInfo) -> Result<&'a mut Self, ProgramError> {
        let mut data = account.try_borrow_mut_data()?;
        if data.len() < Self::SIZE { return Err(ProgramError::InvalidAccountData); }
        let s = bytemuck::try_from_bytes_mut::<Self>(&mut data[..Self::SIZE])
            .map_err(|_| ProgramError::InvalidAccountData)
            .map(|r| unsafe { &mut *(r as *mut Self) })?;
        // 0 is a just-created account the settle callback is about to initialise. Every other
        // writer is gated on a status a zeroed spin cannot have. Reads use `load`, which is strict.
        if s.version != 0 && s.version != VERSION { return Err(ProgramError::InvalidAccountData); }
        Ok(s)
    }

    /// The terms this bet was placed under.
    pub fn terms<'a>(account: &AccountInfo) -> Result<&'a MachineConfig, ProgramError> {
        if account.data_len() < Self::WITH_TERMS {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let data = account.try_borrow()?;
        bytemuck::try_from_bytes::<MachineConfig>(&data[Self::SIZE..Self::WITH_TERMS])
            .map_err(|_| ProgramError::InvalidAccountData)
            .map(|r| unsafe { &*(r as *const MachineConfig) })
    }

    pub fn write_terms(account: &AccountInfo, terms: &MachineConfig) -> ProgramResult {
        if account.data_len() < Self::WITH_TERMS {
            return Err(ProgramError::AccountDataTooSmall);
        }
        let mut data = account.try_borrow_mut_data()?;
        data[Self::SIZE..Self::WITH_TERMS].copy_from_slice(bytemuck::bytes_of(terms));
        Ok(())
    }
}
