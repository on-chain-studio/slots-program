//! The shelf as `scripts/machines.json` describes it — written by the solver
//! (`cargo run --release --example solve` in `engine/`), never by hand — and as the chain holds it.
//!
//! A machine goes on the chain as the program's own `SetMachine`, and is checked against the
//! bytes the program's own builder makes of that same `SetMachine`: the verifier compares every
//! byte of every published machine, not a list of fields someone thought to check.

use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use slots::instructions::set_machine::{InitSymbol, SetMachine};
use slots::state::config::{MachineConfig, GAMBLE_FAIR, MODE_GAMBLE, MODE_HOLD, MODE_LINES};

/// The five paylines every machine on the shelf pays: middle, top, bottom, and the two diagonals.
pub const LINES: [[u8; 3]; 5] = [[1, 1, 1], [0, 0, 0], [2, 2, 2], [0, 1, 2], [2, 1, 0]];
/// Rows shown per reel.
pub const ROWS: u8 = 3;

/// One machine as the solver writes it.
#[derive(Deserialize, Debug, Clone)]
pub struct Machine {
    pub name: String,
    pub mode: String,
    pub stake: u64,
    pub rounds: u8,
    pub rungs: u8,
    /// A ladder rung's chance to win, in percent: 50 is fair, and the shelf shades it below.
    pub win_pct: f64,
    pub mults: Vec<u16>,
    pub strips: Vec<Vec<u8>>,
}

impl Machine {
    pub fn mode(&self) -> Result<u8> {
        Ok(match self.mode.as_str() {
            "lines" => MODE_LINES,
            "hold" => MODE_HOLD,
            "gamble" => MODE_GAMBLE,
            other => bail!("{}: no mode {other:?}", self.name),
        })
    }

    /// The rung's chance out of 2^32, as the program takes it.
    pub fn gamble_win(&self) -> u32 {
        (GAMBLE_FAIR as f64 * (self.win_pct / 50.0)).floor() as u32
    }

    /// The program's `SetMachine` for publishing this machine at `index`.
    pub fn set_machine(&self, index: u8) -> Result<SetMachine> {
        Ok(SetMachine {
            index,
            mode: self.mode()?,
            stake_lamports: self.stake,
            row_count: ROWS,
            rounds: self.rounds,
            gamble_rungs: self.rungs,
            gamble_win: self.gamble_win(),
            // All-zero: native SOL.
            mint: [0; 32],
            strips: self.strips.clone(),
            symbols: self.mults.iter().map(|&mult| InitSymbol { mult, flags: 0 }).collect(),
            lines: LINES.iter().map(|line| line.to_vec()).collect(),
        })
    }

    /// The bytes the program writes for this machine.
    pub fn built(&self, index: u8) -> Result<MachineConfig> {
        let set = self.set_machine(index)?;
        set.validate().map_err(|e| anyhow::anyhow!("{}: the program would refuse it ({e:?})", self.name))?;
        Ok(set.build_for_test())
    }
}

pub fn default_path() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../scripts/machines.json")
}

pub fn load(path: &Path) -> Result<Vec<Machine>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("{} is not a machine sheet", path.display()))
}

/// What differs between a machine on the chain and the one wanted, field by field.
pub fn differences(chain: &MachineConfig, want: &MachineConfig) -> Vec<String> {
    let mut out = Vec::new();
    let mut check = |name: &str, got: String, want: String| {
        if got != want {
            out.push(format!("{name}: chain {got}, sheet {want}"));
        }
    };
    check("stake", chain.stake_lamports.to_string(), want.stake_lamports.to_string());
    check("mode", chain.mode.to_string(), want.mode.to_string());
    check("reels", chain.reel_count.to_string(), want.reel_count.to_string());
    check("strip length", chain.strip_len.to_string(), want.strip_len.to_string());
    check("rows", chain.row_count.to_string(), want.row_count.to_string());
    check("symbols", chain.symbol_count.to_string(), want.symbol_count.to_string());
    check("lines", chain.line_count.to_string(), want.line_count.to_string());
    check("rounds", chain.rounds.to_string(), want.rounds.to_string());
    check("rungs", chain.gamble_rungs.to_string(), want.gamble_rungs.to_string());
    check("gamble win", chain.gamble_win.to_string(), want.gamble_win.to_string());
    check("mint", format!("{:?}", chain.mint), format!("{:?}", want.mint));
    for (r, (got, wanted)) in chain.strips.iter().zip(&want.strips).enumerate() {
        check(&format!("strip {r}"), format!("{got:?}"), format!("{wanted:?}"));
    }
    for (s, (got, wanted)) in chain.symbols.iter().zip(&want.symbols).enumerate() {
        check(&format!("symbol {s}"), format!("×{} flags {}", got.mult, got.flags), format!("×{} flags {}", wanted.mult, wanted.flags));
    }
    for (l, (got, wanted)) in chain.lines.iter().zip(&want.lines).enumerate() {
        check(&format!("line {l}"), format!("{:?}", got.rows), format!("{:?}", wanted.rows));
    }
    // Anything the named fields do not cover — padding included — still counts.
    if out.is_empty() && bytemuck::bytes_of(chain) != bytemuck::bytes_of(want) {
        out.push("bytes differ outside the named fields".into());
    }
    out
}

pub fn mode_name(mode: u8) -> &'static str {
    match mode {
        MODE_LINES => "lines",
        MODE_HOLD => "hold",
        MODE_GAMBLE => "gamble",
        _ => "?",
    }
}
