//! Exactness and parity. Every number here is enumerated, never sampled: a machine with three
//! twenty-stop reels has 8000 spins, so "about right" is never the best available answer.

use slots_engine::*;
use slots_engine::analysis;

/// Five lines: middle, top, bottom, and both diagonals — the prototype's layout.
fn five_lines() -> [Line; MAX_LINES] {
    let mut l = [Line::default(); MAX_LINES];
    let want: [[u8; 3]; 5] = [[1, 1, 1], [0, 0, 0], [2, 2, 2], [0, 1, 2], [2, 1, 0]];
    for (i, rows) in want.iter().enumerate() {
        l[i].rows[..3].copy_from_slice(rows);
    }
    l
}

fn machine(strips: [[u8; 20]; 3], mults: &[u16], mode: u8, rounds: u8) -> MachineConfig {
    let mut m = MachineConfig {
        stake_lamports: 100_000_000,
        mode,
        reel_count: 3,
        strip_len: 20,
        row_count: 3,
        symbol_count: mults.len() as u8,
        line_count: 5,
        rounds,
        lines: five_lines(),
        ..Default::default()
    };
    for (r, s) in strips.iter().enumerate() {
        m.strips[r][..20].copy_from_slice(s);
    }
    for (i, &mult) in mults.iter().enumerate() {
        m.symbols[i] = Symbol { mult, flags: 0, _pad: 0 };
    }
    m
}

/// The UI prototype's GOLD RUSH, exactly as it ships: symbols spread evenly over one strip,
/// reels 2 and 3 rotations of reel 1. Kept as a fixture because its numbers were computed
/// independently in JavaScript from the design, so it cross-checks this enumerator against
/// something that shares no code with it.
fn prototype_gold() -> MachineConfig {
    machine(
        [
            [0, 2, 3, 4, 5, 5, 5, 1, 2, 3, 4, 5, 5, 4, 2, 3, 5, 1, 4, 5],
            [1, 2, 3, 4, 5, 5, 4, 2, 3, 5, 1, 4, 5, 0, 2, 3, 4, 5, 5, 5],
            [4, 2, 3, 5, 1, 4, 5, 0, 2, 3, 4, 5, 5, 5, 1, 2, 3, 4, 5, 5],
        ],
        &[100, 40, 25, 15, 10, 6],
        MODE_LINES,
        1,
    )
}

#[test]
fn enumeration_matches_the_independent_model() {
    let r = analysis::report(&prototype_gold()).unwrap();
    assert_eq!(r.states, 8000);
    // Computed from the design's own logic in JavaScript: 52.5% RTP, 1 in 4.1, top ×28.
    assert!((r.rtp - 0.525).abs() < 0.002, "rtp {}", r.rtp);
    assert!((1.0 / r.hit_rate - 4.1).abs() < 0.05, "hit 1 in {}", 1.0 / r.hit_rate);
    assert!((r.top_multiple - 28.0).abs() < 0.01, "top {}", r.top_multiple);
}

#[test]
fn the_prototype_pay_table_is_not_shippable() {
    // Kept as a standing reminder of why the strips get solved rather than hand-written: the
    // design's own numbers are nowhere near the 80% the house publishes.
    let r = analysis::report(&prototype_gold()).unwrap();
    assert!(r.rtp < 0.60, "prototype was balanced after all — revisit the solver's premise");
}

#[test]
fn a_machine_that_pays_more_than_it_takes_is_visible() {
    // COSMIC 7s from the design, whose enumerated return exceeds 100% — the case a sampled
    // estimate is most likely to wave through.
    let m = machine(
        [
            [0, 2, 3, 4, 5, 5, 5, 1, 2, 3, 4, 5, 5, 4, 2, 3, 5, 1, 4, 5],
            [1, 2, 3, 4, 5, 5, 4, 2, 3, 5, 1, 4, 5, 0, 2, 3, 4, 5, 5, 5],
            [4, 2, 3, 5, 1, 4, 5, 0, 2, 3, 4, 5, 5, 5, 1, 2, 3, 4, 5, 5],
        ],
        &[250, 150, 80, 40, 20, 8],
        MODE_LINES,
        1,
    );
    assert!(analysis::report(&m).unwrap().rtp > 1.0);
}

#[test]
fn holding_is_worth_something_and_never_costs() {
    let mut m = prototype_gold();
    m.mode = MODE_HOLD;
    m.rounds = 3;
    let optimal = analysis::report(&m).unwrap().rtp;
    let naive = analysis::no_hold_rtp(&m).unwrap();
    // A player who never holds gets exactly the one-shot machine; holding well can only help.
    assert!((naive - 0.525).abs() < 0.002, "naive {naive}");
    assert!(optimal > naive * 1.5, "holding bought almost nothing: {optimal} vs {naive}");
}

#[test]
fn more_rounds_never_pay_less() {
    let mut m = prototype_gold();
    m.mode = MODE_HOLD;
    let mut last = 0.0;
    for rounds in 1..=4u8 {
        m.rounds = rounds;
        let rtp = analysis::report(&m).unwrap().rtp;
        assert!(rtp >= last - 1e-9, "{rounds} rounds paid less than {}", rounds - 1);
        last = rtp;
    }
}

#[test]
fn the_stream_does_not_depend_on_the_hold_mask() {
    // The trap this guards: skipping the draw for a held reel would make the same seed produce
    // different reels for different masks, so the client and the chain would agree on a first
    // spin and silently diverge on every respin.
    let m = prototype_gold();
    let seed = [9u8; 32];
    let prev = [7u8, 7, 7, 0, 0];
    let all_free = spin(&m, &seed, 0b000, &prev).unwrap();
    for hold in 0..0b1000u8 {
        let got = spin(&m, &seed, hold, &prev).unwrap();
        for r in 0..3 {
            let want = if hold >> r & 1 == 1 { prev[r] } else { all_free[r] };
            assert_eq!(got[r], want, "reel {r} under mask {hold:03b}");
        }
    }
}

#[test]
fn a_spin_is_the_same_every_time() {
    let m = prototype_gold();
    for i in 0..64u8 {
        let seed = [i; 32];
        let a = evaluate(&m, &seed).unwrap();
        let b = evaluate(&m, &seed).unwrap();
        assert_eq!(a, b);
    }
}

#[test]
fn every_seed_byte_reaches_the_reels() {
    let m = prototype_gold();
    let base = [0u8; 32];
    let first = spin(&m, &base, 0, &[0; MAX_REELS]).unwrap();
    let mut moved = 0;
    for i in 0..32 {
        let mut seed = base;
        seed[i] = 0xA5;
        if spin(&m, &seed, 0, &[0; MAX_REELS]).unwrap() != first {
            moved += 1;
        }
    }
    // Not all 32 must move three small draws, but a seed byte that never can is a bug.
    assert!(moved > 24, "only {moved}/32 seed bytes changed the stops");
}

#[test]
fn stops_are_uniform_over_the_strip() {
    let m = prototype_gold();
    let mut counts = [0u32; 20];
    for i in 0..60_000u32 {
        let mut seed = [0u8; 32];
        seed[..4].copy_from_slice(&i.to_le_bytes());
        counts[spin(&m, &seed, 0, &[0; MAX_REELS]).unwrap()[0] as usize] += 1;
    }
    for (i, &c) in counts.iter().enumerate() {
        assert!((c as i32 - 3000).abs() < 250, "stop {i} drew {c} times");
    }
}

#[test]
fn the_grid_is_three_consecutive_stops() {
    let m = prototype_gold();
    // The wrap is the case worth pinning: a stop at the end of the strip reads round to the front.
    let g = grid(&m, &[19, 0, 0, 0, 0]);
    assert_eq!(g[0][0], m.strips[0][19]);
    assert_eq!(g[0][1], m.strips[0][0]);
    assert_eq!(g[0][2], m.strips[0][1]);
}

#[test]
fn nonsense_machines_are_refused() {
    let ok = prototype_gold();
    assert!(ok.check().is_ok());

    let cases: [(&str, fn(&mut MachineConfig)); 8] = [
        ("no reels", |m| m.reel_count = 0),
        ("too many reels", |m| m.reel_count = MAX_REELS as u8 + 1),
        ("strip longer than the array", |m| m.strip_len = MAX_STRIP as u8 + 1),
        ("more rows than strip", |m| { m.strip_len = 2; m.row_count = 3 }),
        ("no lines", |m| m.line_count = 0),
        ("symbol off the end of the pay table", |m| m.strips[0][0] = 11),
        ("line on a row that isn't shown", |m| m.lines[0].rows[0] = 9),
        ("unknown mode", |m| m.mode = 7),
    ];
    for (name, break_it) in cases {
        let mut m = ok;
        break_it(&mut m);
        assert_eq!(m.check(), Err(BadMachine), "accepted a machine with {name}");
    }
}

#[test]
fn the_ladder_is_never_better_than_fair() {
    let mut m = prototype_gold();
    m.mode = MODE_GAMBLE;
    m.gamble_rungs = 5;

    m.gamble_win = GAMBLE_FAIR;
    assert!(m.check().is_ok(), "an exactly fair ladder must be publishable");

    m.gamble_win = GAMBLE_FAIR + 1;
    assert_eq!(m.check(), Err(BadMachine), "a ladder the player profits from was accepted");

    m.gamble_win = GAMBLE_FAIR;
    m.gamble_rungs = 0;
    assert_eq!(m.check(), Err(BadMachine), "a ladder with no rungs was accepted");
    m.gamble_rungs = 9;
    assert_eq!(m.check(), Err(BadMachine), "an unbounded ladder was accepted");
}

#[test]
fn a_fair_rung_is_a_coin_flip() {
    let mut m = prototype_gold();
    m.mode = MODE_GAMBLE;
    m.gamble_rungs = 5;
    m.gamble_win = GAMBLE_FAIR;

    let mut won = 0;
    for i in 0..40_000u32 {
        let mut seed = [0u8; 32];
        seed[..4].copy_from_slice(&i.to_le_bytes());
        if gamble(&m, &seed).unwrap() {
            won += 1;
        }
    }
    let rate = won as f64 / 40_000.0;
    assert!((rate - 0.5).abs() < 0.01, "fair rung won {rate} of the time");
}

#[test]
fn a_shaded_rung_favours_the_house() {
    let mut m = prototype_gold();
    m.mode = MODE_GAMBLE;
    m.gamble_rungs = 5;
    m.gamble_win = (GAMBLE_FAIR as f64 * 0.96) as u32;

    let mut won = 0;
    for i in 0..40_000u32 {
        let mut seed = [0u8; 32];
        seed[..4].copy_from_slice(&i.to_le_bytes());
        if gamble(&m, &seed).unwrap() {
            won += 1;
        }
    }
    let rate = won as f64 / 40_000.0;
    assert!((rate - 0.48).abs() < 0.01, "shaded rung won {rate} of the time");
}

#[test]
fn parse_reads_back_what_the_account_holds() {
    // Hand-written offsets on purpose: this is the pin that catches the program's struct and the
    // engine's reader drifting apart. If this test needs editing, the account layout changed and
    // every published machine moved with it.
    const AT_MINT: usize = 24;
    const AT_STRIPS: usize = AT_MINT + 32;
    const AT_SYMBOLS: usize = AT_STRIPS + MAX_REELS * MAX_STRIP;
    const AT_LINES: usize = AT_SYMBOLS + MAX_SYMBOLS * 4;
    const AT_MATCH: usize = AT_LINES + MAX_LINES * MAX_REELS;
    const AT_SPANS: usize = AT_MATCH + 8;
    const AT_RUN_PAYS: usize = AT_SPANS + MAX_LINES * 2;
    assert_eq!(AT_RUN_PAYS + MAX_SYMBOLS * MAX_REELS * 2, MACHINE_BYTES);

    let lines = prototype_gold();
    // The same machine paying runs: three-reel spans, so a run is the whole line and the two
    // tables can be told apart only by which one the bytes say to read.
    let mut runs = lines;
    runs.match_rule = MATCH_RUNS;
    for i in 0..runs.line_count as usize {
        runs.runs.spans[i] = Span { start: 0, count: 3 };
    }
    for s in 0..runs.symbol_count as usize {
        runs.runs.pays[s][2] = lines.symbols[s].mult * 2;
    }
    for m in [lines, runs] {
        let mut bytes = vec![0u8; MACHINE_BYTES];
        bytes[..8].copy_from_slice(&m.stake_lamports.to_le_bytes());
        bytes[8] = m.mode;
        bytes[9] = m.reel_count;
        bytes[10] = m.strip_len;
        bytes[11] = m.row_count;
        bytes[12] = m.symbol_count;
        bytes[13] = m.line_count;
        bytes[14] = m.rounds;
        bytes[15] = m.gamble_rungs;
        bytes[16..20].copy_from_slice(&m.gamble_win.to_le_bytes());
        // the mint at AT_MINT stays zero — SOL, and the engine never reads it
        for r in 0..MAX_REELS {
            for i in 0..MAX_STRIP {
                bytes[AT_STRIPS + r * MAX_STRIP + i] = m.strips[r][i];
            }
        }
        for i in 0..MAX_SYMBOLS {
            let at = AT_SYMBOLS + i * 4;
            bytes[at..at + 2].copy_from_slice(&m.symbols[i].mult.to_le_bytes());
            bytes[at + 2] = m.symbols[i].flags;
        }
        for i in 0..MAX_LINES {
            let at = AT_LINES + i * MAX_REELS;
            bytes[at..at + MAX_REELS].copy_from_slice(&m.lines[i].rows);
        }
        bytes[AT_MATCH] = m.match_rule;
        for i in 0..MAX_LINES {
            bytes[AT_SPANS + i * 2] = m.runs.spans[i].start;
            bytes[AT_SPANS + i * 2 + 1] = m.runs.spans[i].count;
        }
        for s in 0..MAX_SYMBOLS {
            for n in 0..MAX_REELS {
                let at = AT_RUN_PAYS + (s * MAX_REELS + n) * 2;
                bytes[at..at + 2].copy_from_slice(&m.runs.pays[s][n].to_le_bytes());
            }
        }

        let back = parse(&bytes).expect("valid machine rejected");
        assert_eq!(back.match_rule, m.match_rule);
        // The whole point of parse: same bytes, same spin, so the client and the chain cannot drift.
        let mut paid = 0;
        for i in 0..64u8 {
            let seed = [i; 32];
            let w = evaluate(&m, &seed).unwrap();
            assert_eq!(w, evaluate(&back, &seed).unwrap());
            paid += w.lamports;
        }
        assert!(paid > 0, "no seed paid, so nothing was compared");
        assert!(parse(&bytes[..MACHINE_BYTES - 1]).is_none(), "short account accepted");
    }
}

#[test]
fn a_runs_machine_pays_its_runs_through_value() {
    // The program and the wasm only ever call `value`; the run table has to be reachable from it.
    let mut c = MachineConfig {
        stake_lamports: 900, reel_count: 5, strip_len: 3, row_count: 3, symbol_count: 2,
        line_count: 1, match_rule: MATCH_RUNS, ..Default::default()
    };
    c.strips = [[0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]; 5];
    c.runs.spans[0] = Span { start: 0, count: 5 };
    c.runs.pays[0] = [0, 0, 10, 30, 100];
    assert_eq!(value(&c, &[1, 0, 0, 0, 1]).unwrap().lamports, 900 * 10);
    assert_eq!(value(&c, &[1, 0, 0, 0, 0]).unwrap().lamports, 900 * 30);
    assert_eq!(value(&c, &[0; 5]).unwrap(), value_runs(&c, &c.runs, &[0; 5]).unwrap().0);

    c.runs.spans[0] = Span { start: 0, count: 2 };
    assert!(value(&c, &[0; 5]).is_err(), "a span too short to hold a run was accepted");
    c.runs.spans[0] = Span { start: 0, count: 5 };
    c.match_rule = 2;
    assert!(value(&c, &[0; 5]).is_err(), "an unknown match rule was accepted");
}
