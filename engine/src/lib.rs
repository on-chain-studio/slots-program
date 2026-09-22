//! The slot engine: draws a stop per reel, reads the grid off the strips, and values the lines.
//!
//! A machine is its **strips** — an ordered run of symbols per reel — and a spin is one stop
//! per reel. The three visible rows are three consecutive strip positions, so a strip's *order*
//! is part of the machine, not just its symbol counts: two alike sitting next to each other
//! change what the diagonals can do. That is why `rtp` enumerates stops rather than multiplying
//! probabilities.
//!
//! The Solana program calls this crate; the client runs it as wasm. Both must derive the same
//! grid from the same seed, or a player watches one spin and is paid for another.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(feature = "std"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

mod rng;
pub use rng::{Rng, TOTAL};

pub const MAX_REELS: usize = 5;
pub const MAX_STRIP: usize = 32;
pub const MAX_ROWS: usize = 3;
pub const MAX_SYMBOLS: usize = 12;
pub const MAX_LINES: usize = 16;

/// Spin, read the lines, done.
pub const MODE_LINES: u8 = 0;
/// Spin, hold any reels, respin — `rounds` times. Only the final grid pays.
pub const MODE_HOLD: u8 = 1;
/// Spin as `MODE_LINES`, then a win may be risked up the ladder, `gamble_rungs` times.
pub const MODE_GAMBLE: u8 = 2;

/// A fair rung: double or nothing at exactly even odds. `gamble_win` may sit below this — never
/// above, or the ladder would pay the player to climb it.
pub const GAMBLE_FAIR: u32 = 1 << 31;

/// One symbol's pay: what a full line of it multiplies the line stake by.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Symbol {
    pub mult: u16,
    pub flags: u8,
    pub _pad: u8,
}

/// One payline: the row it occupies on each reel.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Line {
    pub rows: [u8; MAX_REELS],
}

/// A machine as the config account stores it: its strips, its lines, its pay table.
#[derive(Clone, Copy)]
pub struct MachineConfig {
    pub stake_lamports: u64,
    pub mode: u8,
    pub reel_count: u8,
    pub strip_len: u8,
    pub row_count: u8,
    pub symbol_count: u8,
    pub line_count: u8,
    /// MODE_HOLD: total grids the player sees, so `rounds - 1` respins. Ignored otherwise.
    pub rounds: u8,
    /// MODE_GAMBLE: how many times a win may be doubled. Ignored otherwise.
    pub gamble_rungs: u8,
    /// Chance a rung is won, out of `TOTAL`. At `GAMBLE_FAIR` the ladder is pure variance and
    /// leaves the machine's RTP untouched — the house edge stays in the spin, where it is
    /// published, instead of hiding in a coin flip.
    pub gamble_win: u32,
    /// Symbol index at each stop, per reel. Independent per reel — never rotations of one strip,
    /// which would correlate the reels.
    pub strips: [[u8; MAX_STRIP]; MAX_REELS],
    pub symbols: [Symbol; MAX_SYMBOLS],
    pub lines: [Line; MAX_LINES],
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            stake_lamports: 0,
            mode: MODE_LINES,
            reel_count: 0,
            strip_len: 0,
            row_count: 0,
            symbol_count: 0,
            line_count: 0,
            rounds: 1,
            gamble_rungs: 0,
            gamble_win: GAMBLE_FAIR,
            strips: [[0u8; MAX_STRIP]; MAX_REELS],
            symbols: [Symbol::default(); MAX_SYMBOLS],
            lines: [Line::default(); MAX_LINES],
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct BadMachine;

/// Where each reel came to rest — an index into that reel's strip.
pub type Stops = [u8; MAX_REELS];

/// The visible window: symbol index at `[reel][row]`.
pub type Grid = [[u8; MAX_ROWS]; MAX_REELS];

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Winnings {
    pub lamports: u64,
    /// Bit per payline that paid — what the client lights up.
    pub lines: u32,
}

impl MachineConfig {
    pub fn reels(&self) -> usize {
        (self.reel_count as usize).min(MAX_REELS)
    }
    pub fn rows(&self) -> usize {
        (self.row_count as usize).min(MAX_ROWS)
    }
    pub fn strip(&self) -> usize {
        (self.strip_len as usize).min(MAX_STRIP)
    }
    pub fn lines(&self) -> &[Line] {
        &self.lines[..(self.line_count as usize).min(MAX_LINES)]
    }
    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols[..(self.symbol_count as usize).min(MAX_SYMBOLS)]
    }
    pub fn rounds(&self) -> usize {
        if self.mode == MODE_HOLD { (self.rounds as usize).max(1) } else { 1 }
    }

    /// Every way a published machine can be nonsense. Called before anything reads a strip, so
    /// the hot paths can index without bounds games.
    pub fn check(&self) -> Result<(), BadMachine> {
        if self.mode != MODE_LINES && self.mode != MODE_HOLD && self.mode != MODE_GAMBLE {
            return Err(BadMachine);
        }
        if self.reel_count == 0 || self.reel_count as usize > MAX_REELS {
            return Err(BadMachine);
        }
        if self.strip_len == 0 || self.strip_len as usize > MAX_STRIP {
            return Err(BadMachine);
        }
        if self.row_count == 0 || self.row_count as usize > MAX_ROWS {
            return Err(BadMachine);
        }
        // Three visible rows off a strip shorter than three would show one symbol twice.
        if (self.row_count as usize) > (self.strip_len as usize) {
            return Err(BadMachine);
        }
        if self.symbol_count == 0 || self.symbol_count as usize > MAX_SYMBOLS {
            return Err(BadMachine);
        }
        if self.line_count == 0 || self.line_count as usize > MAX_LINES {
            return Err(BadMachine);
        }
        if self.mode == MODE_HOLD && (self.rounds == 0 || self.rounds > 8) {
            return Err(BadMachine);
        }
        if self.mode == MODE_GAMBLE {
            // A rung doubles, so the cap is what bounds the worst payout the house must hold.
            if self.gamble_rungs == 0 || self.gamble_rungs > 8 {
                return Err(BadMachine);
            }
            // Above fair the ladder would be +EV and every player would climb it forever.
            if self.gamble_win as u64 > GAMBLE_FAIR as u64 {
                return Err(BadMachine);
            }
        }
        for r in 0..self.reels() {
            for i in 0..self.strip() {
                if self.strips[r][i] >= self.symbol_count {
                    return Err(BadMachine);
                }
            }
        }
        for l in self.lines() {
            for r in 0..self.reels() {
                if l.rows[r] as usize >= self.rows() {
                    return Err(BadMachine);
                }
            }
        }
        Ok(())
    }

    /// What one line stakes. The bet splits evenly across the paylines, so a machine's headline
    /// multiple is per *line* — a ×250 line on a five-line machine returns ×50 of the bet.
    fn line_stake(&self) -> u64 {
        self.stake_lamports / (self.line_count as u64).max(1)
    }
}

/// The window each stop exposes: three consecutive strip positions, wrapping.
pub fn grid(card: &MachineConfig, stops: &Stops) -> Grid {
    let mut g = [[0u8; MAX_ROWS]; MAX_REELS];
    let len = card.strip();
    for r in 0..card.reels() {
        for y in 0..card.rows() {
            g[r][y] = card.strips[r][(stops[r] as usize + y) % len];
        }
    }
    g
}

/// Draws a stop for every reel not held.
///
/// A draw happens for **every** reel and is then discarded for held ones, so the stream does not
/// depend on the hold mask. Skipping the draw instead would make the same seed produce different
/// reels for different masks — a parity trap between the client and the chain that only shows up
/// on a respin.
pub fn spin(card: &MachineConfig, seed: &[u8; 32], hold: u8, prev: &Stops) -> Result<Stops, BadMachine> {
    card.check()?;
    let mut rng = Rng::from_bytes(seed);
    let len = card.strip() as u64;
    let mut out = *prev;
    for r in 0..card.reels() {
        let drawn = rng.below(len) as u8;
        if hold >> r & 1 == 0 {
            out[r] = drawn;
        }
    }
    Ok(out)
}

/// What a grid pays. A line pays when every reel shows the same symbol on that line's row.
pub fn value(card: &MachineConfig, stops: &Stops) -> Result<Winnings, BadMachine> {
    card.check()?;
    Ok(value_unchecked(card, stops))
}

/// [value] without re-validating the machine — for the enumerator, which checks once and then
/// asks eight thousand times.
fn value_unchecked(card: &MachineConfig, stops: &Stops) -> Winnings {
    let g = grid(card, stops);
    let stake = card.line_stake();
    let mut w = Winnings::default();
    for (i, l) in card.lines().iter().enumerate() {
        let first = g[0][l.rows[0] as usize];
        if (1..card.reels()).all(|r| g[r][l.rows[r] as usize] == first) {
            let mult = card.symbols[first as usize].mult as u64;
            if mult > 0 {
                w.lamports = w.lamports.saturating_add(stake.saturating_mul(mult));
                w.lines |= 1 << i;
            }
        }
    }
    w
}

/// One whole spin from a seed, for the machines that take a single round.
pub fn evaluate(card: &MachineConfig, seed: &[u8; 32]) -> Result<Winnings, BadMachine> {
    let stops = spin(card, seed, 0, &[0u8; MAX_REELS])?;
    value(card, &stops)
}

// ── the account layout ─────────────────────────────────────────────────────────────────────

/// Bytes one machine occupies in the config account, and therefore what the wasm is handed.
/// The mint sits in the middle of it: this crate never reads it — a machine pays in one currency
/// and the engine only ever counts base units — but the stride has to include it or every machine
/// after the first would be read at the wrong offset.
pub const MACHINE_BYTES: usize = 344;

const AT_MINT: usize = 24;
const AT_STRIPS: usize = AT_MINT + 32;
const AT_SYMBOLS: usize = AT_STRIPS + MAX_REELS * MAX_STRIP;
const AT_LINES: usize = AT_SYMBOLS + MAX_SYMBOLS * 4;

const _: () = assert!(AT_LINES + MAX_LINES * MAX_REELS == MACHINE_BYTES);

fn u16le(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32le(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
fn u64le(b: &[u8], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

/// Reads a machine straight out of the stored account bytes — no second serialisation to keep
/// in step with the program's struct.
pub fn parse(b: &[u8]) -> Option<MachineConfig> {
    if b.len() < MACHINE_BYTES {
        return None;
    }
    let mut m = MachineConfig {
        stake_lamports: u64le(b, 0),
        mode: b[8],
        reel_count: b[9],
        strip_len: b[10],
        row_count: b[11],
        symbol_count: b[12],
        line_count: b[13],
        rounds: b[14],
        gamble_rungs: b[15],
        gamble_win: u32le(b, 16),
        ..Default::default()
    };
    for r in 0..MAX_REELS {
        for i in 0..MAX_STRIP {
            m.strips[r][i] = b[AT_STRIPS + r * MAX_STRIP + i];
        }
    }
    for i in 0..MAX_SYMBOLS {
        let at = AT_SYMBOLS + i * 4;
        m.symbols[i] = Symbol { mult: u16le(b, at), flags: b[at + 2], _pad: 0 };
    }
    for i in 0..MAX_LINES {
        let at = AT_LINES + i * MAX_REELS;
        for r in 0..MAX_REELS {
            m.lines[i].rows[r] = b[at + r];
        }
    }
    m.check().ok()?;
    Some(m)
}

// ── the ladder ─────────────────────────────────────────────────────────────────────────────

/// One rung of the gamble: double or nothing.
///
/// Each rung takes its own seed, because the player decides to climb *after* seeing the rung
/// below. A ladder run off one seed would let the client read every rung before choosing, which
/// is the same disclosure trap the hold machine has between respins.
pub fn gamble(card: &MachineConfig, seed: &[u8; 32]) -> Result<bool, BadMachine> {
    card.check()?;
    if card.mode != MODE_GAMBLE {
        return Err(BadMachine);
    }
    Ok(Rng::from_bytes(seed).weight() < card.gamble_win)
}

// ── analysis ───────────────────────────────────────────────────────────────────────────────

/// Exact machine maths. Not compiled into the program or the wasm — this is what the balancing
/// tool and the tests call.
///
/// Nothing here samples. A machine has `strip_len ^ reel_count` possible spins (8000 for three
/// twenty-stop reels), so every number below is enumerated over all of them.
#[cfg(feature = "std")]
pub mod analysis {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Report {
        /// Expected return as a fraction of the bet. 0.80 is the house standard.
        pub rtp: f64,
        /// Fraction of spins that pay anything.
        pub hit_rate: f64,
        /// Best single spin, as a multiple of the bet — the honest headline, not the line mult.
        pub top_multiple: f64,
        /// How many distinct spins exist.
        pub states: u64,
    }

    fn total_states(card: &MachineConfig) -> u64 {
        (card.strip() as u64).pow(card.reels() as u32)
    }

    /// Unpacks a mixed-radix index into one stop per reel.
    fn stops_at(card: &MachineConfig, mut idx: u64) -> Stops {
        let len = card.strip() as u64;
        let mut s = [0u8; MAX_REELS];
        for r in 0..card.reels() {
            s[r] = (idx % len) as u8;
            idx /= len;
        }
        s
    }

    /// Every spin's payout, indexed the same way as `stops_at`. The machine was checked by the
    /// caller; re-validating it per state was most of the enumerator's cost.
    fn payouts(card: &MachineConfig) -> Vec<u64> {
        (0..total_states(card))
            .map(|i| super::value_unchecked(card, &stops_at(card, i)).lamports)
            .collect()
    }

    /// The index of `stops` with the free reels of `hold` zeroed — every state sharing a held
    /// projection faces exactly the same choice, which is what keeps the DP small.
    fn projection(card: &MachineConfig, stops: &Stops, hold: u8) -> usize {
        let len = card.strip();
        let mut key = 0usize;
        for r in (0..card.reels()).rev() {
            key = key * len + if hold >> r & 1 == 1 { stops[r] as usize } else { 0 };
        }
        key
    }

    /// One round of the hold DP: given the value of every state with `k` respins left, the value
    /// with `k + 1`, assuming the player always picks the best hold.
    ///
    /// For a given mask the average over the respun reels depends only on the held reels, so each
    /// mask is one pass that buckets by that projection — 2^reels passes, not 2^reels × states².
    fn step(card: &MachineConfig, next: &[f64], next_hit: &[f64]) -> (Vec<f64>, Vec<f64>) {
        let n = next.len();
        let masks = 1u8 << card.reels();
        let mut best = vec![f64::NEG_INFINITY; n];
        let mut best_hit = vec![0.0f64; n];

        for hold in 0..masks {
            let mut sum = vec![0.0f64; n];
            let mut hit = vec![0.0f64; n];
            let mut count = vec![0u32; n];
            for i in 0..n {
                let key = projection(card, &stops_at(card, i as u64), hold);
                sum[key] += next[i];
                hit[key] += next_hit[i];
                count[key] += 1;
            }
            for i in 0..n {
                let key = projection(card, &stops_at(card, i as u64), hold);
                let c = count[key] as f64;
                let ev = sum[key] / c;
                if ev > best[i] {
                    best[i] = ev;
                    best_hit[i] = hit[key] / c;
                }
            }
        }
        (best, best_hit)
    }

    /// The machine's exact numbers, enumerated. For a hold machine this is the return under
    /// **optimal play** — the only honest figure, since a player who holds well is the one the
    /// house has to be able to pay.
    pub fn report(card: &MachineConfig) -> Result<Report, BadMachine> {
        card.check()?;
        if card.stake_lamports == 0 {
            return Err(BadMachine);
        }
        let stake = card.stake_lamports as f64;
        let pay = payouts(card);
        let n = pay.len();

        let top = pay.iter().copied().max().unwrap_or(0) as f64 / stake;

        let mut ev: Vec<f64> = pay.iter().map(|&p| p as f64).collect();
        let mut hit: Vec<f64> = pay.iter().map(|&p| if p > 0 { 1.0 } else { 0.0 }).collect();
        for _ in 1..card.rounds() {
            let (e, h) = step(card, &ev, &hit);
            ev = e;
            hit = h;
        }

        let mean = ev.iter().sum::<f64>() / n as f64;
        let hits = hit.iter().sum::<f64>() / n as f64;
        Ok(Report { rtp: mean / stake, hit_rate: hits, top_multiple: top, states: n as u64 })
    }

    /// The obvious hold, which is what a player who has never heard of optimal play actually
    /// does: on a grid that already pays, hold everything; otherwise hold the two reels that
    /// agree on some line — the best-paying pair if several do — and respin the rest; with no
    /// pair, respin everything. The return under this rule is the machine's *default* return,
    /// the one most players get, and the figure the house has to be honest about first.
    pub fn naive_mask(card: &MachineConfig, stops: &Stops) -> u8 {
        let reels = card.reels();
        if super::value_unchecked(card, stops).lamports > 0 {
            return ((1u16 << reels) - 1) as u8;
        }
        let g = super::grid(card, stops);
        let mut best: Option<(u16, u8)> = None;
        for l in card.lines() {
            for a in 0..reels {
                for b in (a + 1)..reels {
                    let sa = g[a][l.rows[a] as usize];
                    if sa != g[b][l.rows[b] as usize] {
                        continue;
                    }
                    let mult = card.symbols[sa as usize].mult;
                    if mult > 0 && best.map_or(true, |(m, _)| mult > m) {
                        best = Some((mult, (1u8 << a) | (1u8 << b)));
                    }
                }
            }
        }
        best.map_or(0, |(_, m)| m)
    }

    /// One round under a fixed policy rather than the best choice: the same bucketing as `step`,
    /// but each state takes the mask the policy names instead of the mask that pays most.
    fn policy_step(
        card: &MachineConfig,
        next: &[f64],
        next_hit: &[f64],
        policy: &dyn Fn(&Stops) -> u8,
    ) -> (Vec<f64>, Vec<f64>) {
        let n = next.len();
        let masks = 1usize << card.reels();
        let mut sum = vec![vec![0.0f64; n]; masks];
        let mut hit = vec![vec![0.0f64; n]; masks];
        let mut count = vec![vec![0u32; n]; masks];
        for i in 0..n {
            let stops = stops_at(card, i as u64);
            for hold in 0..masks {
                let key = projection(card, &stops, hold as u8);
                sum[hold][key] += next[i];
                hit[hold][key] += next_hit[i];
                count[hold][key] += 1;
            }
        }
        let mut ev = vec![0.0f64; n];
        let mut ev_hit = vec![0.0f64; n];
        for i in 0..n {
            let stops = stops_at(card, i as u64);
            let hold = policy(&stops) as usize;
            let key = projection(card, &stops, hold as u8);
            let c = count[hold][key] as f64;
            ev[i] = sum[hold][key] / c;
            ev_hit[i] = hit[hold][key] / c;
        }
        (ev, ev_hit)
    }

    /// The machine's numbers for the default player — `naive_mask` at every decision.
    pub fn naive_report(card: &MachineConfig) -> Result<Report, BadMachine> {
        card.check()?;
        if card.stake_lamports == 0 {
            return Err(BadMachine);
        }
        let stake = card.stake_lamports as f64;
        let pay = payouts(card);
        let n = pay.len();
        let top = pay.iter().copied().max().unwrap_or(0) as f64 / stake;
        let mut ev: Vec<f64> = pay.iter().map(|&p| p as f64).collect();
        let mut hit: Vec<f64> = pay.iter().map(|&p| if p > 0 { 1.0 } else { 0.0 }).collect();
        let policy = |s: &Stops| naive_mask(card, s);
        for _ in 1..card.rounds() {
            let (e, h) = policy_step(card, &ev, &hit, &policy);
            ev = e;
            hit = h;
        }
        let mean = ev.iter().sum::<f64>() / n as f64;
        let hits = hit.iter().sum::<f64>() / n as f64;
        Ok(Report { rtp: mean / stake, hit_rate: hits, top_multiple: top, states: n as u64 })
    }

    /// What the hold is worth: the same machine played by someone who never holds. The gap
    /// between this and `report` is the skill component, and it is the part a naive RTP misses.
    pub fn no_hold_rtp(card: &MachineConfig) -> Result<f64, BadMachine> {
        card.check()?;
        let pay = payouts(card);
        let mean = pay.iter().map(|&p| p as f64).sum::<f64>() / pay.len() as f64;
        Ok(mean / card.stake_lamports as f64)
    }
}

// ── wasm ───────────────────────────────────────────────────────────────────────────────────

/// Shared in/out buffer — no allocator here, so the host writes and reads the result in place.
///
/// In:  `MACHINE_BYTES` exactly as the account stores them, the 32-byte seed, the hold mask,
///      then `MAX_REELS` previous stops (both zero for a first spin).
/// Out: `MAX_REELS` stops, the grid as `MAX_REELS × MAX_ROWS` symbol indices, lamports u64,
///      then the winning-line bitmask u32.
pub const BUF_LEN: usize = MACHINE_BYTES + 32 + 1 + MAX_REELS;

#[cfg(target_arch = "wasm32")]
mod wasm_abi {
    use super::*;

    static mut BUF: [u8; BUF_LEN] = [0; BUF_LEN];

    #[no_mangle]
    pub extern "C" fn buffer() -> *mut u8 {
        core::ptr::addr_of_mut!(BUF) as *mut u8
    }

    #[no_mangle]
    pub extern "C" fn buffer_len() -> i32 {
        BUF_LEN as i32
    }

    /// One rung of the ladder: the machine and this rung's seed are in the buffer, the answer
    /// is the return value — 1 doubles, 0 busts. Nothing is written back.
    #[no_mangle]
    pub extern "C" fn flip_buffer() -> i32 {
        let buf = unsafe { &*core::ptr::addr_of!(BUF) };
        let Some(card) = parse(&buf[..MACHINE_BYTES]) else { return -1 };
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&buf[MACHINE_BYTES..MACHINE_BYTES + 32]);
        match gamble(&card, &seed) {
            Ok(true) => 1,
            Ok(false) => 0,
            Err(_) => -1,
        }
    }

    #[no_mangle]
    pub extern "C" fn spin_buffer() -> i32 {
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(BUF) };
        let Some(card) = parse(&buf[..MACHINE_BYTES]) else { return -1 };

        let mut seed = [0u8; 32];
        seed.copy_from_slice(&buf[MACHINE_BYTES..MACHINE_BYTES + 32]);
        let hold = buf[MACHINE_BYTES + 32];
        let mut prev = [0u8; MAX_REELS];
        prev.copy_from_slice(&buf[MACHINE_BYTES + 33..MACHINE_BYTES + 33 + MAX_REELS]);

        let Ok(stops) = spin(&card, &seed, hold, &prev) else { return -1 };
        let Ok(w) = value(&card, &stops) else { return -1 };
        let g = grid(&card, &stops);

        buf[..MAX_REELS].copy_from_slice(&stops);
        for r in 0..MAX_REELS {
            for y in 0..MAX_ROWS {
                buf[MAX_REELS + r * MAX_ROWS + y] = g[r][y];
            }
        }
        let at = MAX_REELS + MAX_REELS * MAX_ROWS;
        buf[at..at + 8].copy_from_slice(&w.lamports.to_le_bytes());
        buf[at + 8..at + 12].copy_from_slice(&w.lines.to_le_bytes());
        0
    }
}
