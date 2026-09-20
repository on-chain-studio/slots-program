use bytemuck::{Pod, Zeroable};
use solana_program::{account_info::AccountInfo, program_error::ProgramError};

pub const DISCRIMINATOR: u64 = 1;
pub const VERSION:       u64 = 1;

/// Starting shelf size, not a ceiling — `GrowConfig` buys more room.
pub const INITIAL_MACHINES: usize = 4;

/// Fixed bounds inside a machine. Every engine loop is bounded by one of these, and they must
/// match `slots_engine`'s exactly — the engine reads these bytes verbatim.
pub const MAX_REELS:   usize = 5;
pub const MAX_STRIP:   usize = 32;
pub const MAX_ROWS:    usize = 3;
pub const MAX_SYMBOLS: usize = 12;
pub const MAX_LINES:   usize = 16;

pub const MODE_LINES:  u8 = 0;
pub const MODE_HOLD:   u8 = 1;
pub const MODE_GAMBLE: u8 = 2;

/// A rung at exactly even odds. A published ladder may sit below this, never above.
pub const GAMBLE_FAIR: u32 = 1 << 31;
/// The worst a rung may be shaded to — 45%. Deeper than this is a typo, not an edge.
pub const GAMBLE_FLOOR: u32 = ((GAMBLE_FAIR as u64) * 9 / 10) as u32;

/// The most rounds a hold machine may run, and the most rungs a ladder may climb. Both bound
/// what the house can be asked to pay for one bet.
pub const MAX_ROUNDS: u8 = 8;
pub const MAX_RUNGS:  u8 = 8;

/// One symbol's pay: what a full line of it multiplies the line stake by.
#[repr(C)]
#[derive(Pod, Zeroable, Clone, Copy)]
pub struct Symbol {
    pub mult:  u16,
    pub flags: u8,
    pub _pad:  u8,
}

/// One payline: the row it occupies on each reel.
#[repr(C)]
#[derive(Pod, Zeroable, Clone, Copy)]
pub struct Line {
    pub rows: [u8; MAX_REELS],
}

/// One machine on the shelf: its strips, its lines, its pay table. The published odds, on-chain.
///
/// The field order is the wire format — `slots_engine::parse` reads these bytes directly rather
/// than through a second serialisation, so moving a field here silently re-reads every machine.
/// `tests/layout.rs` pins it.
#[repr(C)]
#[derive(Pod, Zeroable, Clone, Copy)]
pub struct MachineConfig {
    pub stake_lamports: u64,
    pub mode:         u8,
    pub reel_count:   u8,
    pub strip_len:    u8,
    pub row_count:    u8,
    pub symbol_count: u8,
    pub line_count:   u8,
    /// MODE_HOLD: grids the player sees, so `rounds - 1` respins.
    pub rounds:       u8,
    /// MODE_GAMBLE: how many times a win may be doubled.
    pub gamble_rungs: u8,
    /// Chance a rung is won, out of 2^32.
    pub gamble_win:   u32,
    pub _pad:         u32,
    /// What this machine bets and pays in. All-zero is native SOL.
    pub mint:    [u8; 32],
    /// Symbol index at each stop, per reel. Independent per reel.
    pub strips:  [[u8; MAX_STRIP]; MAX_REELS],
    pub symbols: [Symbol; MAX_SYMBOLS],
    pub lines:   [Line; MAX_LINES],
}

/// `["config"]` — the head of the shelf; machines follow it packed end to end, cast by offset.
#[repr(C)]
#[derive(Pod, Zeroable, Clone, Copy)]
pub struct Config {
    pub discriminator: u64,
    pub version:       u64,
    pub authority:     [u8; 32],
    pub machine_count: u64,
}

pub const MACHINE_SIZE: usize = size_of::<MachineConfig>();

// Header and stride must stay multiples of 8, or bytemuck rejects the misaligned slice at runtime.
const _: () = assert!(size_of::<Config>() % 8 == 0);
const _: () = assert!(MACHINE_SIZE % 8 == 0);

// Pin the sizes: a field reordered into a padding hole would change the stride and misread machines.
const _: () = assert!(size_of::<Symbol>() == 4);
const _: () = assert!(size_of::<Line>() == 5);
const _: () = assert!(MACHINE_SIZE == 344);
// The engine reads this account's bytes; if the two ever disagree it reads a machine at the
// wrong offset and deals a grid nobody published.
const _: () = assert!(MACHINE_SIZE == slots_engine::MACHINE_BYTES);

impl MachineConfig {
    pub fn reels(&self) -> usize {
        (self.reel_count as usize).min(MAX_REELS)
    }
    pub fn lines(&self) -> &[Line] {
        &self.lines[..(self.line_count as usize).min(MAX_LINES)]
    }
    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols[..(self.symbol_count as usize).min(MAX_SYMBOLS)]
    }
    /// How many grids this machine shows for one bet — one, unless it holds.
    pub fn rounds(&self) -> u64 {
        if self.mode == MODE_HOLD { (self.rounds as u64).max(1) } else { 1 }
    }
    /// Mask of the reels this machine actually has, so a hold can't name one that isn't there.
    pub fn reel_mask(&self) -> u64 {
        (1u64 << self.reels()) - 1
    }
}

impl Config {
    pub const HEADER: usize = size_of::<Self>();

    pub const fn size_for(machines: usize) -> usize { Self::HEADER + machines * MACHINE_SIZE }

    /// How many machines this account has room for — its size, not its contents.
    pub fn capacity(account: &AccountInfo) -> usize {
        account.data_len().saturating_sub(Self::HEADER) / MACHINE_SIZE
    }

    pub fn load<'a>(account: &AccountInfo<'a>) -> Result<&'a Self, ProgramError> {
        let data = account.try_borrow_data()?;
        if data.len() < Self::HEADER { return Err(ProgramError::InvalidAccountData); }
        let s = bytemuck::try_from_bytes::<Self>(&data[..Self::HEADER])
            .map_err(|_| ProgramError::InvalidAccountData)
            .map(|r| unsafe { &*(r as *const Self) })?;
        if s.version != VERSION { return Err(ProgramError::InvalidAccountData); }
        Ok(s)
    }

    pub fn load_mut<'a>(account: &AccountInfo<'a>) -> Result<&'a mut Self, ProgramError> {
        let mut data = account.try_borrow_mut_data()?;
        if data.len() < Self::HEADER { return Err(ProgramError::InvalidAccountData); }
        // Not version-checked: this is the write path where `Initialize` sets the version.
        bytemuck::try_from_bytes_mut::<Self>(&mut data[..Self::HEADER])
            .map_err(|_| ProgramError::InvalidAccountData)
            .map(|r| unsafe { &mut *(r as *mut Self) })
    }

    /// A published machine, for playing — bounded by `machine_count`, not capacity.
    pub fn machine<'a>(
        account: &AccountInfo<'a>,
        machine_id: u64,
    ) -> Result<&'a MachineConfig, ProgramError> {
        if machine_id >= Self::load(account)?.machine_count {
            return Err(crate::error::GameError::InvalidMachine.into());
        }
        Self::slot(account, machine_id as usize)
    }

    /// A slot, for writing — bounded by capacity, since this is how a machine gets published.
    pub fn slot<'a>(
        account: &AccountInfo<'a>,
        index: usize,
    ) -> Result<&'a MachineConfig, ProgramError> {
        let (from, to) = Self::span(account, index)?;
        let data = account.try_borrow_data()?;
        bytemuck::try_from_bytes::<MachineConfig>(&data[from..to])
            .map_err(|_| ProgramError::InvalidAccountData)
            .map(|r| unsafe { &*(r as *const MachineConfig) })
    }

    pub fn slot_mut<'a>(
        account: &AccountInfo<'a>,
        index: usize,
    ) -> Result<&'a mut MachineConfig, ProgramError> {
        let (from, to) = Self::span(account, index)?;
        let mut data = account.try_borrow_mut_data()?;
        bytemuck::try_from_bytes_mut::<MachineConfig>(&mut data[from..to])
            .map_err(|_| ProgramError::InvalidAccountData)
            .map(|r| unsafe { &mut *(r as *mut MachineConfig) })
    }

    fn span(account: &AccountInfo, index: usize) -> Result<(usize, usize), ProgramError> {
        if index >= Self::capacity(account) {
            return Err(crate::error::GameError::InvalidMachine.into());
        }
        let from = Self::HEADER + index * MACHINE_SIZE;
        Ok((from, from + MACHINE_SIZE))
    }
}
