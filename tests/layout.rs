//! The seam between the program and the engine.
//!
//! The engine reads a machine's stored bytes directly, with no shared struct and no second
//! serialisation. That is fast and drift-proof in one direction only: if these two layouts ever
//! disagree, every machine after the first is read at the wrong offset and the chain deals a grid
//! nobody published. These tests are the thing standing in the way of that.

use bytemuck::Zeroable;
use slots::instructions::set_machine::{InitSymbol, SetMachine, MAX_BET_MULTIPLE};
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
    }
}

#[test]
fn the_stride_matches_the_engine() {
    assert_eq!(MACHINE_SIZE, slots_engine::MACHINE_BYTES);
    assert_eq!(MACHINE_SIZE, 344);
    assert_eq!(std::mem::size_of::<Config>(), 56);
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
fn zeroed_bytes_are_not_a_playable_machine() {
    // A grown shelf is zero-filled, and `machine_count` is what makes a slot playable. If a
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
    // Position in the instructions! list is the wire discriminator. These are the numbers every
    // client is built against; if this test needs editing, old clients are already broken.
    use slots::instruction::ix;
    assert_eq!(ix::Initialize, 1);
    assert_eq!(ix::Delegate, 2);
    assert_eq!(ix::Undelegate, 3);
    assert_eq!(ix::RequestUndelegation, 4);
    assert_eq!(ix::SetMachine, 5);
    assert_eq!(ix::GrowConfig, 6);
    assert_eq!(ix::CloseSpin, 7);
    assert_eq!(ix::OpenLedger, 8);
    assert_eq!(ix::DelegateTreasury, 9);
    assert_eq!(ix::UndelegateTreasury, 10);
    assert_eq!(ix::CloseLedger, 11);
    assert_eq!(ix::AuthorizeTreasury, 12);
    assert_eq!(ix::SetPrivacy, 13);
    assert_eq!(ix::WithdrawHouse, 14);
    assert_eq!(ix::RequestBet, 16);
    assert_eq!(ix::ResolveBet, 17);
    assert_eq!(ix::RequestReveal, 18);
    assert_eq!(ix::CallbackReveal, 19);
    assert_eq!(ix::Hold, 20);
    assert_eq!(ix::Gamble, 21);
    assert_eq!(ix::RequestCollect, 22);
    assert_eq!(ix::ResolveCollect, 23);
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
