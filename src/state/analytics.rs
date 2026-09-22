use bytemuck::{Pod, Zeroable};
use crate::chain::*;

pub const DISCRIMINATOR: u64 = 4;
pub const VERSION:       u64 = 1;

/// Room for per-machine counters and payout rows. Caps, not the live shelf: the shelf can grow
/// past these, and a bet or payout beyond them still moves the money — only the count is
/// dropped, because analytics must never be the reason a bet or a collect fails.
pub const MACHINE_SLOTS: usize = 16;
pub const TOKEN_SLOTS:   usize = 16;

#[repr(C)]
#[derive(Pod, Zeroable, Clone, Copy)]
pub struct PayoutRow {
    pub mint:   [u8; 32],
    pub amount: u64,
}

/// `["analytics"]` — lifetime money counters. Written only by the settle callbacks, so every
/// number is settled money rather than a request that might still fail. Delegated to the rollup
/// alongside the house PDA, since that is where the callbacks run.
#[repr(C)]
#[derive(Pod, Zeroable, Clone, Copy)]
pub struct Analytics {
    pub discriminator: u64,
    pub version:       u64,
    /// Gross stakes taken, in each machine's own base units folded together — exact while the
    /// shelf is SOL-only, indicative once token machines exist (the per-mint rows stay exact).
    pub lamports_in:   u64,
    pub bets_placed:    [u64; MACHINE_SLOTS],
    pub bets_collected: [u64; MACHINE_SLOTS],
    /// Lifetime payouts per mint; SOL is the all-zero mint.
    pub payouts:        [PayoutRow; TOKEN_SLOTS],
}

impl Analytics {
    pub const SIZE: usize = size_of::<Self>();

    pub fn load_mut<'a>(account: &AccountInfo) -> Result<&'a mut Self, ProgramError> {
        let mut data = account.try_borrow_mut_data()?;
        if data.len() < Self::SIZE { return Err(ProgramError::InvalidAccountData); }
        let s = bytemuck::try_from_bytes_mut::<Self>(&mut data[..Self::SIZE])
            .map_err(|_| ProgramError::InvalidAccountData)
            .map(|r| unsafe { &mut *(r as *mut Self) })?;
        if s.version != 0 && s.version != VERSION { return Err(ProgramError::InvalidAccountData); }
        Ok(s)
    }

    /// Adds to a mint's lifetime payout, claiming the first free row for a mint not seen before.
    /// SOL's row keeps the all-zero mint, which is also what an unclaimed row looks like — the
    /// amount being set is what marks it claimed, so the match on mint runs first.
    pub fn record_payout(&mut self, mint: &[u8; 32], amount: u64) {
        for row in self.payouts.iter_mut() {
            if row.mint == *mint {
                row.amount = row.amount.saturating_add(amount);
                return;
            }
            if row.amount == 0 && row.mint == [0u8; 32] {
                row.mint = *mint;
                row.amount = amount;
                return;
            }
        }
    }

    pub fn count(slot: &mut u64) {
        *slot = slot.saturating_add(1);
    }
}
