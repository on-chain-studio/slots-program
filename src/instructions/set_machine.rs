use borsh::BorshDeserialize;
use bytemuck::Zeroable;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::constants::is_admin;
use crate::error::GameError;
use crate::instruction::ProcessInstruction;
use crate::state::config::*;
use crate::utils::pda;

/// The worst single bet the house will ever be asked to pay, as a multiple of the stake.
///
/// Not a design limit — it is a typo guard, and it has to be tight enough to actually catch one.
/// Every line landing the top symbol pays `stake × mult`, and a ladder doubles that once per rung,
/// so a mistyped multiplier or one rung too many is the difference between a machine and an
/// unpayable liability. At ×10,000 a 0.1 SOL bet can still cost the house 1000 SOL, which is far
/// past anything the shelf should carry — the runbook sizes the real float, this only refuses the
/// absurd. Set above ~65,000 it would stop catching a `u16::MAX` multiplier at all.
pub const MAX_BET_MULTIPLE: u64 = 10_000;

#[derive(BorshDeserialize)]
pub struct InitSymbol {
    pub mult:  u16,
    pub flags: u8,
}

/// Writes one machine of the public shelf: its strips, its lines, its pay table. Admin only.
/// One machine per transaction.
/// Accounts: [admin (signer), config]
#[derive(BorshDeserialize)]
pub struct SetMachine {
    pub index:          u8,
    pub mode:           u8,
    pub stake_lamports: u64,
    pub row_count:      u8,
    pub rounds:         u8,
    pub gamble_rungs:   u8,
    pub gamble_win:     u32,
    pub mint:           [u8; 32],
    /// One strip per reel, each `strip_len` symbol indices. Reel count and strip length are read
    /// from this rather than passed, so they can never disagree with it.
    pub strips:         Vec<Vec<u8>>,
    pub symbols:        Vec<InitSymbol>,
    /// One row index per reel, per line.
    pub lines:          Vec<Vec<u8>>,
}

const BAD: ProgramError = ProgramError::InvalidInstructionData;

fn need(ok: bool) -> Result<(), ProgramError> {
    if ok { Ok(()) } else { Err(BAD) }
}

impl SetMachine {
    /// Everything the engine is allowed to assume, plus everything an admin can plausibly
    /// mistype. A published machine is played from a copy of these bytes, so a shape the engine
    /// rejects at spin time would strand a bet that has already been paid for.
    pub fn validate(&self) -> Result<(), ProgramError> {
        need(self.mode == MODE_LINES || self.mode == MODE_HOLD || self.mode == MODE_GAMBLE)?;
        need(self.stake_lamports > 0)?;

        // ── shape
        need(!self.strips.is_empty() && self.strips.len() <= MAX_REELS)?;
        need(!self.symbols.is_empty() && self.symbols.len() <= MAX_SYMBOLS)?;
        need(!self.lines.is_empty() && self.lines.len() <= MAX_LINES)?;
        need(self.row_count as usize >= 1 && self.row_count as usize <= MAX_ROWS)?;

        let reels = self.strips.len();
        let strip_len = self.strips[0].len();
        need(strip_len >= 1 && strip_len <= MAX_STRIP)?;
        // Every reel the same length: the stop is one index and the window wraps within it.
        need(self.strips.iter().all(|s| s.len() == strip_len))?;
        // Three rows off a strip shorter than three would show the same symbol twice in a column.
        need(strip_len >= self.row_count as usize)?;

        // ── the strips only name symbols that exist
        need(self
            .strips
            .iter()
            .all(|s| s.iter().all(|&i| (i as usize) < self.symbols.len())))?;

        // ── a machine that can never pay is a machine nobody should be sold
        need(self.symbols.iter().any(|s| s.mult > 0))?;

        // ── lines land on rows that are actually shown
        for l in &self.lines {
            need(l.len() == reels)?;
            need(l.iter().all(|&r| (r as usize) < self.row_count as usize))?;
        }
        // A duplicated payline pays the same win twice and silently doubles the machine's return.
        // Always a publishing mistake, never a design.
        for (i, l) in self.lines.iter().enumerate() {
            need(!self.lines[..i].iter().any(|p| p == l))?;
        }

        // ── mode-specific
        match self.mode {
            MODE_HOLD => {
                need(self.rounds >= 2 && self.rounds <= MAX_ROUNDS)?;
            }
            MODE_GAMBLE => {
                need(self.gamble_rungs >= 1 && self.gamble_rungs <= MAX_RUNGS)?;
                // The rungs are shaded: part of the house edge is taken on the flip, not only on
                // the reels. Above fair would pay the player to climb forever; below the floor is
                // a typo, not a shading — no intended edge puts a coin flip under 45%.
                need(self.gamble_win <= GAMBLE_FAIR)?;
                need(self.gamble_win >= GAMBLE_FLOOR)?;
            }
            _ => {}
        }

        // ── what the house can be asked for
        //
        // Every line landing the top symbol at once is reachable: it is simply every reel showing
        // it. So the worst bet is `stake × max_mult`, doubled once per ladder rung.
        let top = self.symbols.iter().map(|s| s.mult as u64).max().unwrap_or(0);
        let rungs = if self.mode == MODE_GAMBLE { self.gamble_rungs as u32 } else { 0 };
        let worst = top.checked_shl(rungs).ok_or(BAD)?;
        need(worst <= MAX_BET_MULTIPLE)?;
        need(self.stake_lamports.checked_mul(worst.max(1)).is_some())?;

        Ok(())
    }

    #[doc(hidden)]
    pub fn build_for_test(&self) -> MachineConfig { self.build() }

    fn build(&self) -> MachineConfig {
        let mut m = MachineConfig::zeroed();
        m.stake_lamports = self.stake_lamports;
        m.mode = self.mode;
        m.reel_count = self.strips.len() as u8;
        m.strip_len = self.strips[0].len() as u8;
        m.row_count = self.row_count;
        m.symbol_count = self.symbols.len() as u8;
        m.line_count = self.lines.len() as u8;
        m.rounds = if self.mode == MODE_HOLD { self.rounds } else { 1 };
        m.gamble_rungs = if self.mode == MODE_GAMBLE { self.gamble_rungs } else { 0 };
        m.gamble_win = if self.mode == MODE_GAMBLE { self.gamble_win } else { GAMBLE_FAIR };
        m.mint = self.mint;
        for (r, strip) in self.strips.iter().enumerate() {
            m.strips[r][..strip.len()].copy_from_slice(strip);
        }
        for (i, s) in self.symbols.iter().enumerate() {
            m.symbols[i] = Symbol { mult: s.mult, flags: s.flags, _pad: 0 };
        }
        for (i, l) in self.lines.iter().enumerate() {
            m.lines[i].rows[..l.len()].copy_from_slice(l);
        }
        m
    }
}

impl ProcessInstruction for SetMachine {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [admin, config_account, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        if !admin.is_signer || !is_admin(admin.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        pda::validate(program_id, config_account, &[b"config"])?;
        self.validate()?;

        let built = self.build();
        // The last word on whether this is publishable belongs to the engine that will play it,
        // not to the checks above — they can drift, `parse` cannot.
        if slots_engine::parse(bytemuck::bytes_of(&built)).is_none() {
            return Err(GameError::InvalidMachine.into());
        }

        let index = self.index as usize;
        *Config::slot_mut(config_account, index)? = built;

        // A machine written past the count publishes it. The count never shrinks.
        let c = Config::load_mut(config_account)?;
        c.machine_count = c.machine_count.max(index as u64 + 1);

        Ok(())
    }
}
