//! Dumps parity vectors for the Kotlin client: the published machines from
//! scripts/machines.json, each spun from deterministic seeds, with the stops, grid, payout and
//! line mask the chain would compute — plus hold respins and ladder flips. The Android test
//! pins `slots_engine.wasm` against this file, so the reels the player watches and the money
//! the chain settles cannot disagree.
//!
//!   cargo run --release --example vectors > ../../slots/app/src/test/resources/vectors.json

use slots_engine::*;

/// The same spread the Kotlin side uses: word w of seed n is `n * (GOLDEN + w)`, LE.
fn seed_bytes(n: u64) -> [u8; 32] {
    let mut out = [0u8; 32];
    for w in 0..4u64 {
        let v = n.wrapping_mul(0x9e3779b97f4a7c15u64.wrapping_add(w));
        out[w as usize * 8..][..8].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// The account form, mirroring `parse` — the bytes the config stores and the wasm is handed.
fn account_bytes(m: &MachineConfig) -> Vec<u8> {
    let mut b = vec![0u8; MACHINE_BYTES];
    b[..8].copy_from_slice(&m.stake_lamports.to_le_bytes());
    b[8] = m.mode;
    b[9] = m.reel_count;
    b[10] = m.strip_len;
    b[11] = m.row_count;
    b[12] = m.symbol_count;
    b[13] = m.line_count;
    b[14] = m.rounds;
    b[15] = m.gamble_rungs;
    b[16..20].copy_from_slice(&m.gamble_win.to_le_bytes());
    // mint 24..56 stays zero (SOL)
    for r in 0..MAX_REELS {
        b[56 + r * MAX_STRIP..][..MAX_STRIP].copy_from_slice(&m.strips[r]);
    }
    let at_sym = 56 + MAX_REELS * MAX_STRIP;
    for i in 0..MAX_SYMBOLS {
        b[at_sym + i * 4..][..2].copy_from_slice(&m.symbols[i].mult.to_le_bytes());
        b[at_sym + i * 4 + 2] = m.symbols[i].flags;
    }
    let at_lines = at_sym + MAX_SYMBOLS * 4;
    for i in 0..MAX_LINES {
        b[at_lines + i * MAX_REELS..][..MAX_REELS].copy_from_slice(&m.lines[i].rows);
    }
    assert!(parse(&b).is_some(), "writer drifted from parse");
    b
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn main() {
    let json = std::fs::read_to_string("../scripts/machines.json").expect("machines.json");
    let sheet: serde_json::Value = serde_json::from_str(&json).unwrap();

    println!("[");
    let machines = sheet.as_array().unwrap();
    for (mi, mj) in machines.iter().enumerate() {
        let mode = match mj["mode"].as_str().unwrap() {
            "hold" => MODE_HOLD,
            "gamble" => MODE_GAMBLE,
            _ => MODE_LINES,
        };
        let strips_j = mj["strips"].as_array().unwrap();
        let mults_j = mj["mults"].as_array().unwrap();
        let mut m = MachineConfig {
            stake_lamports: mj["stake"].as_u64().unwrap(),
            mode,
            reel_count: strips_j.len() as u8,
            strip_len: strips_j[0].as_array().unwrap().len() as u8,
            row_count: 3,
            symbol_count: mults_j.len() as u8,
            line_count: 5,
            rounds: if mode == MODE_HOLD { mj["rounds"].as_u64().unwrap() as u8 } else { 1 },
            gamble_rungs: if mode == MODE_GAMBLE { mj["rungs"].as_u64().unwrap() as u8 } else { 0 },
            gamble_win: if mode == MODE_GAMBLE {
                ((1u64 << 31) * mj["win_pct"].as_u64().unwrap() / 50) as u32
            } else {
                1 << 31
            },
            ..Default::default()
        };
        for (i, rows) in [[1, 1, 1], [0, 0, 0], [2, 2, 2], [0, 1, 2], [2, 1, 0]].iter().enumerate() {
            m.lines[i].rows[..3].copy_from_slice(rows);
        }
        for (r, s) in strips_j.iter().enumerate() {
            for (i, v) in s.as_array().unwrap().iter().enumerate() {
                m.strips[r][i] = v.as_u64().unwrap() as u8;
            }
        }
        for (i, v) in mults_j.iter().enumerate() {
            m.symbols[i].mult = v.as_u64().unwrap() as u16;
        }

        let bytes = account_bytes(&m);
        let mut rows = Vec::new();
        for n in 0..24u64 {
            let seed = seed_bytes(mi as u64 * 1000 + n);
            // First round: everything free. Then, for a hold machine, a respin holding reel 0
            // — the hold path the client renders most.
            let s1 = spin(&m, &seed, 0, &[0; MAX_REELS]).unwrap();
            let w1 = value(&m, &s1).unwrap();
            let mut row = format!(
                "{{ \"seed\": {}, \"stops\": {:?}, \"lamports\": {}, \"lines\": {}",
                mi as u64 * 1000 + n, &s1[..m.reels()], w1.lamports, w1.lines,
            );
            if mode == MODE_HOLD {
                let seed2 = seed_bytes(mi as u64 * 1000 + n + 500);
                let s2 = spin(&m, &seed2, 0b001, &s1).unwrap();
                let w2 = value(&m, &s2).unwrap();
                row += &format!(
                    ", \"respinSeed\": {}, \"respinStops\": {:?}, \"respinLamports\": {}",
                    mi as u64 * 1000 + n + 500, &s2[..m.reels()], w2.lamports,
                );
            }
            if mode == MODE_GAMBLE {
                row += &format!(", \"flip\": {}", gamble(&m, &seed).unwrap());
            }
            row += " }";
            rows.push(row);
        }
        println!(
            "  {{ \"name\": {}, \"machine\": \"{}\",\n    \"spins\": [\n      {}\n    ] }}{}",
            mj["name"], hex(&bytes), rows.join(",\n      "),
            if mi + 1 < machines.len() { "," } else { "" },
        );
    }
    println!("]");
}
