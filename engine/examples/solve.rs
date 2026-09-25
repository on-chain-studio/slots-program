//! Solves the shelf: strips per machine, held to the house targets under each mode's real
//! maths — plain enumeration for lines, the optimal-play DP for hold, the shaded ladder riding
//! on a lines machine for gamble. Writes ../scripts/machines.json for `slots-ops publish`.
//!
//!   cargo run --release --example solve
//!
//! The pay tables are **fixed**: each machine declares its prizes as clean whole-bet multiples
//! (x2, x5, x10…), and the line multipliers derive from them (per-bet x line count). What the
//! solver owns is the *odds* — symbol counts per reel, independently, and their placement —
//! which is where all the freedom lives anyway: probabilities are products across three reels,
//! a far finer trim than any integer multiplier tweak. The RTP target is 90% on the spin: no
//! jackpot exists, so the whole return lives in the machines. The ladder is excluded from the
//! target — its rungs are published at 48%, a premium the climber pays by choice.

use slots_engine::{analysis, Line, MachineConfig, Symbol, MAX_LINES, MODE_HOLD, MODE_LINES};

/// The house standard for a machine with no decisions. The hold machine sets its own two
/// figures (see its Goal): what it returns to a player who never holds, and what it returns
/// to one who holds well.
const TARGET_RTP: f64 = 0.90;
/// The layout's cap, used in full: 32 stops per dial is a /32768 lattice over three reels —
/// the granularity that lets fixed clean prizes land on 90.00 without integer strain.
const STRIP: usize = 32;
const REELS: usize = 3;
/// Seven paying symbols, the commonest at x2: every win nets something. A x1 floor bought a
/// higher hit rate by handing the bet back, and it read as a machine that never pays — most of
/// the return sat in that one symbol while anything above x5 came once in hundreds of spins.
const SYMS: usize = 7;

/// Deterministic pseudo-random stream for placement and search moves — reruns give the same shelf.
struct Det(u64);
impl Det {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 11
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Greedy anti-run placement: at every stop, the symbol with the most copies left that does
/// not put three alike in a row (wrap included) — most-copies-first is what keeps a heavy
/// common feasible to the end, and the reel phase varies the tie-breaks so the reels are
/// genuinely different orders. The x1 floor made the commonest symbol nearly half the strip,
/// which broke the old spread-then-squeeze placer into visible runs.
fn place(counts: &[usize; SYMS], reel: usize) -> [u8; STRIP] {
    let mut left = *counts;
    let mut strip = [u8::MAX; STRIP];
    for i in 0..STRIP {
        let prev1 = if i >= 1 { strip[i - 1] } else { u8::MAX };
        let prev2 = if i >= 2 { strip[i - 2] } else { u8::MAX };
        let mut pick = usize::MAX;
        for off in 0..SYMS {
            // phase the scan per reel and position so equal counts break ties differently
            let s = (off + reel * 2 + i) % SYMS;
            if left[s] == 0 {
                continue;
            }
            // no third-in-a-row, and don't let the last stops close a run around the wrap
            if prev1 == s as u8 && prev2 == s as u8 {
                continue;
            }
            if i == STRIP - 1 && strip[0] == s as u8 && strip[1] == s as u8 && prev1 == s as u8 {
                continue;
            }
            if pick == usize::MAX || left[s] > left[pick] {
                pick = s;
            }
        }
        if pick == usize::MAX {
            // cornered (a symbol left with nowhere legal) — place anything; score() rejects runs
            pick = (0..SYMS).max_by_key(|&s| left[s]).unwrap();
        }
        strip[i] = pick as u8;
        left[pick] -= 1;
    }
    strip
}

/// Three alike in a row anywhere on a strip, wrap included.
fn has_run(strip: &[u8; STRIP]) -> bool {
    (0..STRIP).any(|i| {
        strip[i] == strip[(i + 1) % STRIP] && strip[i] == strip[(i + 2) % STRIP]
    })
}

/// Grids per bet on the hold machine — `rounds - 1` respins. Overridable while exploring, since
/// the number of respins is what decides how much holding is worth over never holding.
fn hold_rounds() -> u8 {
    std::env::var("SLOTS_HOLD_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(3)
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn machine(counts: &[[usize; SYMS]; REELS], rots: &[usize; REELS], mults: &[u16; SYMS], mode: u8) -> MachineConfig {
    let mut lines = [Line::default(); MAX_LINES];
    for (i, rows) in [[1, 1, 1], [0, 0, 0], [2, 2, 2], [0, 1, 2], [2, 1, 0]].iter().enumerate() {
        lines[i].rows[..3].copy_from_slice(rows);
    }
    let mut m = MachineConfig {
        stake_lamports: 5_000_000,
        mode,
        reel_count: REELS as u8,
        strip_len: STRIP as u8,
        row_count: 3,
        symbol_count: SYMS as u8,
        line_count: 5,
        rounds: if mode == MODE_HOLD { hold_rounds() } else { 1 },
        lines,
        ..Default::default()
    };
    for r in 0..REELS {
        let base = place(&counts[r], r);
        for i in 0..STRIP {
            m.strips[r][i] = base[(i + rots[r]) % STRIP];
        }
    }
    for (i, &mult) in mults.iter().enumerate() {
        m.symbols[i] = Symbol { mult, flags: 0, _pad: 0 };
    }
    m
}

struct Goal {
    name: &'static str,
    mode: u8,
    /// The prizes, as clean whole-bet multiples — fixed, never searched.
    bet_mults: [u16; SYMS],
    /// hit-rate window under the mode's own play (optimal, for hold)
    hit: (f64, f64),
    /// Cap on the last (floor) step of the ladder. A frequent machine keeps most of its RTP
    /// and hit rate in a very common floor symbol — capping that step chokes it below any
    /// reachable RTP target — so only a machine that can afford a bounded floor sets one.
    floor_cap: f64,
    /// The return under the mode's own play — optimal holds, for hold.
    rtp: f64,
    /// Hold only: the return for the default player, who holds the obvious way — two reels
    /// agree, spin the third (`analysis::naive_mask`). "Optimal play" means nothing to most
    /// players, so the machine has to be honest at the default too — the base pays a real
    /// return, and holding well lifts it to `rtp`.
    base_rtp: Option<f64>,
}

const LINES: u16 = 5;

/// Adjacent-tier frequency bounds for line modes, on the across-reels count product.
const RATIO_MIN: f64 = 1.25;
const RATIO_MAX: f64 = 6.0;

fn score(g: &Goal, counts: &[[usize; SYMS]; REELS], rots: &[usize; REELS], mults: &[u16; SYMS]) -> Option<f64> {
    // structural sanity per reel, and a rarer symbol never likelier than a commoner one —
    // measured on the product across reels, which is what a line actually rolls.
    for c in counts {
        if c.iter().any(|&x| x == 0) || c.iter().sum::<usize>() != STRIP {
            return None;
        }
    }
    // Line modes fence the ladder's *shape*: each step down the paytable must be at least
    // RATIO_MIN likelier and at most RATIO_MAX — equal-frequency tiers and cliffs both read
    // as broken odds. A graded penalty rather than a rejection, because the valid region is
    // thin and a climb has to be able to walk through it; the winner is re-checked hard in
    // main. Hold keeps the bare ordering: its effective frequencies come out of the hold DP,
    // not the raw products, so this fence would be the wrong one.
    let mut shape = 0.0;
    for i in 1..SYMS {
        let p_prev = counts.iter().map(|c| c[i - 1]).product::<usize>() as f64;
        let p_this = counts.iter().map(|c| c[i]).product::<usize>() as f64;
        if g.mode == MODE_HOLD {
            if p_this < p_prev {
                return None;
            }
        } else {
            let cap = if i == SYMS - 1 { g.floor_cap } else { RATIO_MAX };
            let (lo, hi) = (p_prev * RATIO_MIN, p_prev * cap);
            if p_this < lo { shape += (lo - p_this) / lo; }
            if p_this > hi { shape += (p_this - hi) / hi; }
        }
    }
    // A cornered placement leaks a run; a strip with one is not a candidate.
    for r in 0..REELS {
        if has_run(&place(&counts[r], r)) {
            return None;
        }
    }
    let r = analysis::report(&machine(counts, rots, mults, g.mode)).ok()?;
    // Weighted so even a fraction-of-a-percent shape violation outranks any reachable RTP
    // improvement — the fence bends during the walk but never in the answer.
    let mut s = shape * 2000.0;
    if let Some(base) = g.base_rtp {
        // Two figures, weighted by which one is being anchored (env, while exploring). The
        // ceiling is a cliff rather than a wall: a search that rejected every over-paying
        // machine outright had nowhere to step from its start and never moved.
        let (top_w, base_w) = (env_f64("SLOTS_HOLD_TOP_WEIGHT", 200.0), env_f64("SLOTS_HOLD_BASE_WEIGHT", 400.0));
        s += (r.rtp - g.rtp).abs() * top_w;
        if r.rtp > 0.995 {
            s += (r.rtp - 0.995) * 20_000.0;
        }
        let naive = analysis::naive_report(&machine(counts, rots, mults, g.mode)).ok()?.rtp;
        s += (naive - base).abs() * base_w;
    } else {
        s += (r.rtp - g.rtp).abs() * 200.0;
    }
    if r.hit_rate < g.hit.0 { s += (g.hit.0 - r.hit_rate) * 60.0; }
    if r.hit_rate > g.hit.1 { s += (r.hit_rate - g.hit.1) * 60.0; }
    Some(s)
}

fn solve(g: &Goal, seed: u64) -> ([[usize; SYMS]; REELS], [usize; REELS], [u16; SYMS]) {
    let mults = g.bet_mults.map(|m| m * LINES);
    // Restarts, because a single hill-climb can wedge in a basin — the hold machine's
    // landscape especially, where the DP folds every mask choice into each score. Rotations
    // are the fine knob: shifting a strip moves the window correlations without touching the
    // marginals, which is what trims the last tenths of a percent.
    let mut best_counts = [[2usize, 3, 4, 5, 5, 6, 7]; REELS];
    let mut best_rots = [0usize; REELS];
    let mut best = f64::MAX;
    // Lines machines evaluate in microseconds; the hold DP in milliseconds. Spend accordingly —
    // seven symbols fragment the monotone-product constraint into many invalid single moves,
    // and the cure for a starved neighborhood is simply more attempts through it.
    let (restarts, iters) = if g.mode == MODE_HOLD { (4u64, 30_000) } else { (8u64, 30_000) };
    for restart in 0..restarts {
        let mut rng = Det(seed ^ (restart.wrapping_mul(0x9E37)));
        // Alternate basins: a greedy climb never *reaches* the frequent-hits region from a
        // mid-weighted start (every step toward it overshoots RTP before frequency pays off),
        // so half the restarts simply begin there.
        let mut counts = if restart % 2 == 0 {
            [[2usize, 3, 4, 5, 5, 6, 7]; REELS]
        } else {
            [[1usize, 2, 3, 3, 4, 5, 14]; REELS]
        };
        let mut rots = [0usize; REELS];
        let mut local = score(g, &counts, &rots, &mults).unwrap_or(f64::MAX);
        for _ in 0..iters {
            let mut c = counts;
            let mut ro = rots;
            // Compound moves: raising a common's frequency raises RTP unless a rare gets rarer
            // in the same step, and a single move can never do both — the ridge the frequent
            // basin sits behind is only walkable two or three moves at a time.
            let moves = 1 + rng.below(3);
            for _ in 0..moves {
                if rng.below(3) < 2 {
                    let r = rng.below(REELS);
                    let (a, b) = (rng.below(SYMS), rng.below(SYMS));
                    if a == b || c[r][a] <= 1 {
                        continue;
                    }
                    c[r][a] -= 1;
                    c[r][b] += 1;
                } else {
                    let r = rng.below(REELS);
                    ro[r] = (ro[r] + 1 + rng.below(STRIP - 1)) % STRIP;
                }
            }
            if let Some(s) = score(g, &c, &ro, &mults) {
                if s < local {
                    local = s;
                    counts = c;
                    rots = ro;
                }
            }
        }
        if local < best {
            best = local;
            best_counts = counts;
            best_rots = rots;
        }
    }
    (best_counts, best_rots, mults)
}

fn main() {
    // The published prizes, in whole-bet multiples. What each machine *is*, chosen by hand;
    // the solver only decides how often.
    let goals = [
        // At a x2 floor, hit x average-win must fit under 0.9, so the hit windows sit lower than
        // a bet-back floor allowed — and every one of those hits is a win.
        Goal { name: "GOLD RUSH",    mode: MODE_LINES, bet_mults: [50, 20, 10, 6, 4, 3, 2],   hit: (0.22, 0.32), floor_cap: f64::INFINITY, rtp: TARGET_RTP, base_rtp: None },
        // The hold machine pays two figures, both published: 75% to a player who never holds,
        // 98% to one who holds well — the skill is worth 23 points and the expert nearly
        // breaks even. Holds convert near-misses into mid-ladder wins, so the temper is the
        // chase itself: three decisions per bet. The window only fences pathology.
        Goal { name: "GRAVITY WELL", mode: MODE_HOLD,  bet_mults: [20, 10, 6, 5, 4, 3, 2],    hit: (0.20, 0.40), floor_cap: f64::INFINITY, rtp: 0.98, base_rtp: Some(0.75) },
    ];

    // Solve each machine once. The stake never enters the solve — RTP is a ratio — so a machine
    // is published at two stakes below, low and high, from one set of strips.
    let solved: Vec<MachineConfig> = goals
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let (counts, rots, mults) = solve(g, 0x5107 + i as u64);
            if g.mode != MODE_HOLD {
                for s in 1..SYMS {
                    let p_prev = counts.iter().map(|c| c[s - 1]).product::<usize>() as f64;
                    let p_this = counts.iter().map(|c| c[s]).product::<usize>() as f64;
                    let cap = if s == SYMS - 1 { g.floor_cap } else { RATIO_MAX };
                    assert!(
                        p_this >= p_prev * RATIO_MIN && p_this <= p_prev * cap,
                        "{}: tier {} product {} outside [{}, {}] of previous {}",
                        g.name, s, p_this, p_prev * RATIO_MIN, p_prev * cap, p_prev,
                    );
                }
            }
            let m = machine(&counts, &rots, &mults, g.mode);
            let r = analysis::report(&m).unwrap();
            print!("{:13} rtp {:6.2}%  hit 1 in {:4.1}  top x{:4.1}", g.name, r.rtp * 100.0, 1.0 / r.hit_rate, r.top_multiple);
            if g.mode == MODE_HOLD {
                print!("  (obvious holds {:.1}%; never holding {:.1}%)",
                    analysis::naive_report(&m).unwrap().rtp * 100.0, analysis::no_hold_rtp(&m).unwrap() * 100.0);
            }
            println!();
            m
        })
        .collect();

    // The shelf, in on-chain id order: each solved machine at a low and a high stake. The `name`
    // is a label for the client (which supplies the theme); the chain carries only the math. Goal
    // 0 is the lines machine, goal 1 the hold machine.
    let shelf: [(&str, usize, u64); 4] = [
        ("NEON NIGHTS",  0,  5_000_000),
        ("GOLD RUSH",    0, 50_000_000),
        ("GRAVITY WELL", 1,  5_000_000),
        ("BLACK HOLE",   1, 50_000_000),
    ];

    let mut out = String::from("[\n");
    for (i, (label, goal, stake)) in shelf.iter().enumerate() {
        let g = &goals[*goal];
        let m = &solved[*goal];
        let strips: Vec<String> = (0..REELS)
            .map(|r| format!("[{}]", m.strips[r][..STRIP].iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",")))
            .collect();
        let mults: Vec<String> = m.symbols[..SYMS].iter().map(|s| s.mult.to_string()).collect();
        out.push_str(&format!(
            "  {{ \"name\": \"{}\", \"mode\": \"{}\", \"stake\": {}, \"rounds\": {}, \"rungs\": 0, \"win_pct\": 50,\n    \"mults\": [{}],\n    \"strips\": [{}] }}{}\n",
            label,
            if g.mode == MODE_HOLD { "hold" } else { "lines" },
            stake,
            if g.mode == MODE_HOLD { hold_rounds() } else { 1 },
            mults.join(","),
            strips.join(", "),
            if i + 1 < shelf.len() { "," } else { "" },
        ));
    }
    out.push_str("]\n");
    let path = std::env::var("SLOTS_OUT").unwrap_or_else(|_| "../scripts/machines.json".to_string());
    std::fs::write(&path, &out).unwrap();
    println!("\nwrote {path}");
}
