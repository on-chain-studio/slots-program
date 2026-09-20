//! Prints a machine's exact numbers. `cargo run --release --example report`
use slots_engine::{analysis, Line, MachineConfig, Symbol, MAX_LINES, MODE_HOLD, MODE_LINES};

fn main() {
    let mut lines = [Line::default(); MAX_LINES];
    for (i, rows) in [[1, 1, 1], [0, 0, 0], [2, 2, 2], [0, 1, 2], [2, 1, 0]].iter().enumerate() {
        lines[i].rows[..3].copy_from_slice(rows);
    }
    let strips: [[u8; 20]; 3] = [
        [0, 2, 3, 4, 5, 5, 5, 1, 2, 3, 4, 5, 5, 4, 2, 3, 5, 1, 4, 5],
        [1, 2, 3, 4, 5, 5, 4, 2, 3, 5, 1, 4, 5, 0, 2, 3, 4, 5, 5, 5],
        [4, 2, 3, 5, 1, 4, 5, 0, 2, 3, 4, 5, 5, 5, 1, 2, 3, 4, 5, 5],
    ];
    let mut m = MachineConfig {
        stake_lamports: 100_000_000,
        mode: MODE_LINES,
        reel_count: 3, strip_len: 20, row_count: 3, symbol_count: 6, line_count: 5, rounds: 1,
        lines, ..Default::default()
    };
    for (r, s) in strips.iter().enumerate() { m.strips[r][..20].copy_from_slice(s); }
    for (i, &mult) in [100u16, 40, 25, 15, 10, 6].iter().enumerate() {
        m.symbols[i] = Symbol { mult, flags: 0, _pad: 0 };
    }

    let r = analysis::report(&m).unwrap();
    println!("LINES    rtp {:5.1}%  hit 1 in {:4.1}  top x{:.0}   ({} spins enumerated)",
        r.rtp * 100.0, 1.0 / r.hit_rate, r.top_multiple, r.states);

    m.mode = MODE_HOLD;
    for rounds in 2..=4u8 {
        m.rounds = rounds;
        let r = analysis::report(&m).unwrap();
        println!("HOLD {rounds}   rtp {:5.1}%  hit 1 in {:4.1}  top x{:.0}   (optimal play)",
            r.rtp * 100.0, 1.0 / r.hit_rate, r.top_multiple);
    }
    println!("         no-hold baseline {:.1}%", analysis::no_hold_rtp(&m).unwrap() * 100.0);
}
