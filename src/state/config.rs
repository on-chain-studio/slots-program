use bytemuck::{Pod, Zeroable};
use casino_core::shelf::Shelf;

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

/// `["config"]` — the shelf: a `casino_core::shelf::Header`, then the MachineConfigs it publishes packed
/// end to end, cast by offset.
pub type Config = Shelf<MachineConfig, VERSION>;

pub const MACHINE_SIZE: usize = size_of::<MachineConfig>();

// The stride must stay a multiple of 8, or bytemuck rejects the misaligned slice at runtime.
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
