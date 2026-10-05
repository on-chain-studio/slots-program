//! The seam between the program and the engine.
//!
//! The engine reads a machine's stored bytes directly, with no shared struct and no second
//! serialisation. That is fast and drift-proof in one direction only: if these two layouts ever
//! disagree, every machine after the first is read at the wrong offset and the chain deals a grid
//! nobody published. These tests are the thing standing in the way of that.

use bytemuck::Zeroable;
use slots::instructions::set_machine::{InitRuns, InitSymbol, SetMachine, MAX_BET_MULTIPLE};
use slots::state::config::*;

fn strip(pattern: &[u8], len: usize) -> Vec<u8> {
    (0..len).map(|i| pattern[i % pattern.len()]).collect()
}

fn gold() -> SetMachine {
    SetMachine {
        index: 0,
        mode: MODE_LINES,
        stake_lamports: 100_000_000,
        row_count: 3,
        rounds: 1,
        gamble_rungs: 0,
        gamble_win: GAMBLE_FAIR,
        mint: [0u8; 32],
        strips: vec![
            strip(&[0, 2, 3, 4, 5, 5, 5, 1, 2, 3, 4, 5, 5, 4, 2, 3, 5, 1, 4, 5], 20),
            strip(&[1, 2, 3, 4, 5, 5, 4, 2, 3, 5, 1, 4, 5, 0, 2, 3, 4, 5, 5, 5], 20),
            strip(&[4, 2, 3, 5, 1, 4, 5, 0, 2, 3, 4, 5, 5, 5, 1, 2, 3, 4, 5, 5], 20),
        ],
        symbols: [100u16, 40, 25, 15, 10, 6]
            .iter()
            .map(|&mult| InitSymbol { mult, flags: 0 })
            .collect(),
        lines: vec![vec![1, 1, 1], vec![0, 0, 0], vec![2, 2, 2], vec![0, 1, 2], vec![2, 1, 0]],
        shown_in: 0,
        runs: None,
    }
}

/// Five reels paying runs: three rows across and six three-reel diagonals, each read only over
/// its own span. Every reel the same `0 1 2 3` strip, so a stop of zero lines up whole rows.
fn five() -> SetMachine {
    let strip = strip(&[0, 1, 2, 3], 16);
    let mut lines = vec![vec![0; 5], vec![1; 5], vec![2; 5]];
    let mut spans = vec![(0, 5); 3];
    for start in 0..3 {
        for rows in [[0, 1, 2], [2, 1, 0]] {
            let mut line = vec![1; 5];
            line[start..start + 3].copy_from_slice(&rows);
            lines.push(line);
            spans.push((start as u8, 3));
        }
    }
    SetMachine {
        index: 0,
        mode: MODE_LINES,
        stake_lamports: 90_000_000,
        row_count: 3,
        rounds: 1,
        gamble_rungs: 0,
        gamble_win: GAMBLE_FAIR,
        mint: [0u8; 32],
        strips: vec![strip; 5],
        symbols: (0..4).map(|_| InitSymbol { mult: 0, flags: 0 }).collect(),
        lines,
        shown_in: SHOWN_IN_CASINO,
        runs: Some(InitRuns {
            spans,
            pays: (1..=4u16).map(|s| vec![0, 0, 9 * s, 27 * s, 90 * s]).collect(),
        }),
    }
}

#[test]
fn the_stride_matches_the_engine() {
    assert_eq!(MACHINE_SIZE, slots_engine::MACHINE_BYTES);
    assert_eq!(MACHINE_SIZE, 504);
    assert_eq!(Config::HEADER, 56);
    assert_eq!(std::mem::size_of::<slots::state::Spin>(), 160);
    // 24 header + 16+16 machine counters + 16 payout rows of 40.
    assert_eq!(std::mem::size_of::<slots::state::Analytics>(), 24 + 32 * 8 + 16 * 40);
}

#[test]
fn a_published_machine_is_what_the_engine_reads() {
    let m = gold();
    m.validate().expect("gold rush rejected");
    let built = m.build_for_test();

    let parsed = slots_engine::parse(bytemuck::bytes_of(&built)).expect("engine rejected it");
    assert_eq!(parsed.reel_count, 3);
    assert_eq!(parsed.strip_len, 20);
    assert_eq!(parsed.symbol_count, 6);
    assert_eq!(parsed.line_count, 5);
    assert_eq!(parsed.symbols[0].mult, 100);
    assert_eq!(parsed.lines[4].rows[..3], [2, 1, 0]);
    for r in 0..3 {
        assert_eq!(parsed.strips[r][..20], built.strips[r][..20]);
    }

    // And the numbers survive the round trip: this is the same machine whose RTP the engine's
    // own tests pin at 52.5% against an independent model.
    let report = slots_engine::analysis::report(&parsed).unwrap();
    assert!((report.rtp - 0.525).abs() < 0.002, "rtp {}", report.rtp);
}

#[test]
fn a_second_machine_is_read_at_the_right_offset() {
    // The failure this guards is silent: a wrong stride still parses, just as the neighbouring
    // machine. So publish two that differ and prove the second reads back as itself.
    let mut shelf = vec![0u8; MACHINE_SIZE * 2];

    let a = gold().build_for_test();
    let mut b = a;
    b.stake_lamports = 250_000_000;
    b.symbols[0].mult = 77;

    shelf[..MACHINE_SIZE].copy_from_slice(bytemuck::bytes_of(&a));
    shelf[MACHINE_SIZE..].copy_from_slice(bytemuck::bytes_of(&b));

    let read_a = slots_engine::parse(&shelf[..MACHINE_SIZE]).unwrap();
    let read_b = slots_engine::parse(&shelf[MACHINE_SIZE..]).unwrap();
    assert_eq!(read_a.symbols[0].mult, 100);
    assert_eq!(read_b.symbols[0].mult, 77);
    assert_eq!(read_b.stake_lamports, 250_000_000);
}

#[test]
fn a_runs_machine_is_paid_by_its_runs() {
    let m = five();
    m.validate().expect("the five-reel machine was rejected");
    let built = m.build_for_test();
    let parsed = slots_engine::parse(bytemuck::bytes_of(&built)).expect("engine rejected it");
    assert_eq!(parsed.match_rule, slots_engine::MATCH_RUNS);
    assert_eq!((parsed.runs.spans[3].start, parsed.runs.spans[3].count), (0, 3));
    assert_eq!(parsed.runs.pays[2], [0, 0, 27, 81, 270]);

    // A line's stake is a ninth of 0.09 SOL.
    let line = 10_000_000;
    // Stop zero: the three rows are five-runs of symbols 0, 1 and 2; no diagonal runs.
    let w = slots_engine::value(&parsed, &[0; 5]).unwrap();
    assert_eq!(w.lamports, line * (90 + 180 + 270));
    assert_eq!(w.lines, 0b111);
    // Knock the first reel out of line and every row becomes a four-run from the second reel.
    let w = slots_engine::value(&parsed, &[1, 0, 0, 0, 0]).unwrap();
    assert_eq!(w.lamports, line * (27 + 54 + 81));
    // Two reels out, and the rows are three-runs over the last three.
    let w = slots_engine::value(&parsed, &[1, 1, 0, 0, 0]).unwrap();
    assert_eq!(w.lamports, line * (9 + 18 + 27));
    // The middle reel out breaks every row into runs of two, which pay nothing.
    assert_eq!(slots_engine::value(&parsed, &[0, 0, 1, 0, 0]).unwrap().lamports, 0);
}

#[test]
fn nonsense_run_tables_are_refused() {
    assert!(five().validate().is_ok());

    let cases: [(&str, fn(&mut InitRuns)); 6] = [
        ("a span missing for a line", |r| { r.spans.pop(); }),
        ("a span too short to hold a run", |r| r.spans[3] = (0, 2)),
        ("a span past the last reel", |r| r.spans[3] = (3, 3)),
        ("a pay for a run of two", |r| r.pays[0][1] = 5),
        ("a pay row the wrong length", |r| { r.pays[0].pop(); }),
        ("no run that pays", |r| r.pays.iter_mut().flatten().for_each(|p| *p = 0)),
    ];
    for (name, break_it) in cases {
        let mut m = five();
        break_it(m.runs.as_mut().unwrap());
        assert!(m.validate().is_err(), "accepted a run table with {name}");
    }

    // The worst bet is the top run on every line at once.
    let mut m = five();
    m.runs.as_mut().unwrap().pays[3][4] = u16::MAX;
    assert!(m.validate().is_err(), "a run pay the house could never hold was accepted");

    // Two collinear spans would pay one run twice. Only the engine sees that, so it is the
    // parse that refuses it.
    let mut m = five();
    m.lines[3] = vec![1, 1, 1, 0, 0];
    m.validate().expect("validate is not where overlap is caught");
    assert!(slots_engine::parse(bytemuck::bytes_of(&m.build_for_test())).is_none());
}

#[test]
fn zeroed_bytes_are_not_a_playable_machine() {
    // A grown shelf is zero-filled, and the header's `count` is what makes a slot playable. If a
    // zeroed slot ever parsed, growing the shelf would publish silent free machines.
    let empty = MachineConfig::zeroed();
    assert!(slots_engine::parse(bytemuck::bytes_of(&empty)).is_none());
}

#[test]
fn nonsense_machines_are_refused() {
    assert!(gold().validate().is_ok());

    let cases: [(&str, fn(&mut SetMachine)); 12] = [
        ("no strips", |m| m.strips.clear()),
        ("ragged strips", |m| m.strips[1].pop().map(|_| ()).unwrap_or(())),
        ("a symbol index off the end of the pay table", |m| m.strips[0][0] = 9),
        ("no symbols that pay", |m| m.symbols.iter_mut().for_each(|s| s.mult = 0)),
        ("a line on a row that isn't shown", |m| m.lines[0][0] = 7),
        ("a line with the wrong reel count", |m| m.lines[0] = vec![1, 1]),
        ("a duplicated payline", |m| m.lines[1] = vec![1, 1, 1]),
        ("no stake", |m| m.stake_lamports = 0),
        ("more rows than the strip is long", |m| {
            m.strips = m.strips.iter().map(|s| s[..2].to_vec()).collect();
            m.row_count = 3;
        }),
        ("a hold machine with one round", |m| { m.mode = MODE_HOLD; m.rounds = 1 }),
        ("a ladder better than fair", |m| {
            m.mode = MODE_GAMBLE;
            m.gamble_rungs = 4;
            m.gamble_win = GAMBLE_FAIR + 1;
        }),
        ("a payout the house could never hold", |m| m.symbols[0].mult = u16::MAX),
    ];
    for (name, break_it) in cases {
        let mut m = gold();
        break_it(&mut m);
        assert!(m.validate().is_err(), "accepted a machine with {name}");
    }
}

#[test]
fn the_ladder_is_counted_into_the_worst_bet() {
    // A ×250 top line is fine on its own and ruinous with eight rungs behind it — the guard has
    // to see the ladder, not just the pay table.
    let mut m = gold();
    m.symbols[0].mult = 250;
    assert!(m.validate().is_ok(), "a x250 machine should publish");

    m.mode = MODE_GAMBLE;
    m.gamble_rungs = 8;
    assert!(m.validate().is_err(), "x250 doubled eight times was accepted");
    assert!(250u64 << 8 > MAX_BET_MULTIPLE / 2, "the fixture stopped exercising the cap");

    m.gamble_rungs = 4;
    assert!(m.validate().is_ok(), "x250 with four rungs should still publish");
}

#[test]
fn the_wire_discriminators_never_move() {
    // The little-endian u64 an instruction starts with. These are the numbers every client is
    // built against, and the settle and VRF callbacks are registered with; if this test needs
    // editing, old clients are already broken.
    use slots::SlotsInstruction as ix;
    assert_eq!(ix::INITIALIZE, 1);
    assert_eq!(ix::DELEGATE, 2);
    assert_eq!(ix::UNDELEGATE, 3);
    assert_eq!(ix::REQUEST_UNDELEGATION, 4);
    assert_eq!(ix::SET_MACHINE, 5);
    assert_eq!(ix::GROW_CONFIG, 6);
    assert_eq!(ix::CLOSE_SPIN, 7);
    assert_eq!(ix::OPEN_LEDGER, 8);
    assert_eq!(ix::DELEGATE_TREASURY, 9);
    assert_eq!(ix::UNDELEGATE_TREASURY, 10);
    assert_eq!(ix::CLOSE_LEDGER, 11);
    assert_eq!(ix::AUTHORIZE_TREASURY, 12);
    assert_eq!(ix::SET_PRIVACY, 13);
    assert_eq!(ix::WITHDRAW_HOUSE, 14);
    assert_eq!(ix::REQUEST_BET, 16);
    assert_eq!(ix::RESOLVE_BET, 17);
    assert_eq!(ix::REQUEST_REVEAL, 18);
    assert_eq!(ix::CALLBACK_REVEAL, 19);
    assert_eq!(ix::HOLD, 20);
    assert_eq!(ix::GAMBLE, 21);
    assert_eq!(ix::REQUEST_COLLECT, 22);
    assert_eq!(ix::RESOLVE_COLLECT, 23);
    assert_eq!(ix::UPGRADE_PERMISSIONS, 24);
}

mod wire {
    use slots::{Slots, SlotsInstruction};

    fn input(discriminator: u64, arguments: &[u8]) -> Vec<u8> {
        [&discriminator.to_le_bytes()[..], arguments].concat()
    }

    #[test]
    fn the_retired_and_reserved_numbers_reach_nothing() {
        for unused in [0, 15, 25, 255] {
            assert!(Slots::instruction(&input(unused, &[])).is_err(), "{unused} was answered");
        }
        assert!(Slots::instruction(&[20, 0, 0, 0]).is_err(), "a short tag was answered");
    }

    #[test]
    fn the_delegation_program_reaches_undelegate_by_its_own_tag() {
        // `sha256("global:process_undelegation")[..8]`, fixed by the delegation program.
        let tag = u64::from_le_bytes([196, 28, 41, 206, 48, 37, 51, 167]);
        let seeds = borsh::to_vec(&vec![b"house".to_vec()]).unwrap();
        for discriminator in [3, tag] {
            let SlotsInstruction::Undelegate(args) =
                Slots::instruction(&input(discriminator, &seeds)).unwrap()
            else {
                panic!("{discriminator} did not reach undelegate");
            };
            assert_eq!(args.args.pda_seeds, vec![b"house".to_vec()]);
        }
    }

    #[test]
    fn a_settle_callback_may_carry_bytes_past_its_arguments() {
        let human = [7u8; 32];
        let consenter = [9u8; 32];
        let arguments = [&human[..], &2u64.to_le_bytes(), &consenter[..], &[0xAA; 16]].concat();
        let SlotsInstruction::ResolveBet(bet) =
            Slots::instruction(&input(17, &arguments)).unwrap()
        else {
            panic!("17 did not reach resolve_bet");
        };
        assert_eq!(bet.args.human.to_bytes(), human);
        assert_eq!(bet.args.machine_id, 2);
        assert_eq!(bet.args.consenter.to_bytes(), consenter);
    }

    #[test]
    fn the_oracle_answer_binds_round_and_bet_generation() {
        let arguments = [&[5u8; 32][..], &3u64.to_le_bytes(), &4u64.to_le_bytes()].concat();
        let SlotsInstruction::CallbackReveal(reveal) =
            Slots::instruction(&input(19, &arguments)).unwrap()
        else {
            panic!("19 did not reach callback_reveal");
        };
        assert_eq!((reveal.args.randomness, reveal.args.round, reveal.args.generation), ([5; 32], 3, 4));
    }

    #[test]
    fn a_hold_carries_its_mask() {
        let SlotsInstruction::Hold(hold) = Slots::instruction(&input(20, &[0b101])).unwrap() else {
            panic!("20 did not reach hold");
        };
        assert_eq!(hold.args.mask, 0b101);
    }
}

#[test]
fn a_shaded_rung_publishes_and_a_scam_rung_does_not() {
    let mut m = gold();
    m.mode = MODE_GAMBLE;
    m.gamble_rungs = 4;

    m.gamble_win = GAMBLE_FAIR;                    // exactly fair
    assert!(m.validate().is_ok());
    m.gamble_win = (GAMBLE_FAIR / 100) * 96;       // 48% — the intended shading
    assert!(m.validate().is_ok());
    m.gamble_win = GAMBLE_FLOOR;                   // 45% — the deepest allowed
    assert!(m.validate().is_ok());
    m.gamble_win = GAMBLE_FLOOR - 1;               // past it: a typo, not a shading
    assert!(m.validate().is_err());
    m.gamble_win = GAMBLE_FAIR / 50;               // 1% — a scam rung
    assert!(m.validate().is_err());
}
