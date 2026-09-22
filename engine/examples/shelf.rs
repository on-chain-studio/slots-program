//! The published shelf's exact numbers — and their shape. `report` gives the return; this says
//! where the return comes from, which is what a pay table feels like to play.
//! `cargo run --release --example shelf`

use slots_engine::analysis;
use slots_engine::{value, Line, MachineConfig, Symbol, MAX_LINES, MAX_REELS, MODE_GAMBLE, MODE_HOLD, MODE_LINES};

fn main() {
    let path = std::env::var("SLOTS_IN").unwrap_or_else(|_| "../scripts/machines.json".to_string());
    let text = std::fs::read_to_string(&path).expect("machines json");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let list = json.as_array().cloned().unwrap_or_else(|| json["machines"].as_array().cloned().unwrap());

    for m in &list {
        let mode = match m["mode"].as_str().unwrap() {
            "hold" => MODE_HOLD,
            "gamble" => MODE_GAMBLE,
            _ => MODE_LINES,
        };
        let strips: Vec<Vec<u8>> = m["strips"].as_array().unwrap().iter()
            .map(|s| s.as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u8).collect()).collect();
        let mults: Vec<u16> = m["mults"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u16).collect();

        let mut lines = [Line::default(); MAX_LINES];
        for (i, rows) in [[1, 1, 1], [0, 0, 0], [2, 2, 2], [0, 1, 2], [2, 1, 0]].iter().enumerate() {
            lines[i].rows[..3].copy_from_slice(rows);
        }
        let mut card = MachineConfig {
            stake_lamports: m["stake"].as_u64().unwrap(),
            mode,
            reel_count: strips.len() as u8,
            strip_len: strips[0].len() as u8,
            row_count: 3,
            symbol_count: mults.len() as u8,
            line_count: 5,
            rounds: m["rounds"].as_u64().unwrap_or(1) as u8,
            gamble_rungs: m["rungs"].as_u64().unwrap_or(0) as u8,
            gamble_win: ((m["win_pct"].as_f64().unwrap_or(50.0) / 100.0) * 4294967296.0) as u32,
            lines,
            ..Default::default()
        };
        for (r, s) in strips.iter().enumerate() {
            card.strips[r][..s.len()].copy_from_slice(s);
        }
        for (i, &mult) in mults.iter().enumerate() {
            card.symbols[i] = Symbol { mult, flags: 0, _pad: 0 };
        }

        let stake = card.stake_lamports as f64;
        let reels = strips.len();
        let len = strips[0].len() as u64;
        let states = len.pow(reels as u32);
        let mut sum = 0.0;
        let mut floor_sum = 0.0;
        let mut hits = 0u64;
        let mut ge2 = 0u64;
        let mut ge5 = 0u64;
        let mut ge10 = 0u64;
        let mut top = 0u64;
        for idx in 0..states {
            let mut stops = [0u8; MAX_REELS];
            let mut i = idx;
            for r in 0..reels {
                stops[r] = (i % len) as u8;
                i /= len;
            }
            let pay = value(&card, &stops).unwrap().lamports;
            sum += pay as f64;
            if pay > 0 { hits += 1; }
            if pay as f64 == stake { floor_sum += pay as f64; }
            if pay as f64 >= 2.0 * stake { ge2 += 1; }
            if pay as f64 >= 5.0 * stake { ge5 += 1; }
            if pay as f64 >= 10.0 * stake { ge10 += 1; }
            top = top.max(pay);
        }
        let n = states as f64;
        let base_rtp = sum / n / stake;
        let report = analysis::report(&card).unwrap();

        println!("\n{}  ({}, stake {} SOL)", m["name"].as_str().unwrap(), m["mode"].as_str().unwrap(), stake / 1e9);
        match mode {
            MODE_HOLD => {
                let naive = analysis::naive_report(&card).unwrap();
                println!(
                    "  return  {:.1}% holding the obvious way (two agree, spin the third)   {:.1}% under optimal holds   {:.1}% never holding",
                    naive.rtp * 100.0, report.rtp * 100.0, base_rtp * 100.0);
                println!("  per bet, obvious holds: anything 1 in {:.1}", 1.0 / naive.hit_rate);
            }
            MODE_GAMBLE => println!(
                "  return  {:.1}% banking every win   {:.1}% after {} rungs at {}%",
                base_rtp * 100.0,
                base_rtp * 100.0 * (2.0 * card.gamble_win as f64 / 4294967296.0).powi(card.gamble_rungs as i32),
                card.gamble_rungs, m["win_pct"]),
            _ => println!("  return  {:.1}%", base_rtp * 100.0),
        }
        println!("  of the base return, {:.0}% is the ×1 bet-back symbol", floor_sum / sum * 100.0);
        println!("  per spin (no holds):  anything 1 in {:.1}   ≥×2 1 in {:.0}   ≥×5 1 in {:.0}   ≥×10 1 in {:.0}   top ×{}",
            n / hits as f64, n / ge2.max(1) as f64, n / ge5.max(1) as f64, n / ge10.max(1) as f64, top as f64 / stake);
        if mode == MODE_HOLD {
            println!("  per bet (optimal holds, {} grids): anything 1 in {:.1}", card.rounds, 1.0 / report.hit_rate);
        }
    }
}
