use crate::{grid, BadMachine, MachineConfig, Stops, Winnings, MAX_LINES, MAX_REELS, MAX_SYMBOLS};

/// A contiguous section of an existing payline. Columns outside it do not participate.
#[derive(Clone, Copy, Default, Debug)]
pub struct Span {
    pub start: u8,
    pub count: u8,
}

/// Optional run payouts, independent of the legacy account format and full-line rules.
#[derive(Clone, Copy)]
pub struct RunRules {
    pub spans: [Span; MAX_LINES],
    /// Multiplier by symbol and exact run length minus one. Zero means no payout.
    pub pays: [[u16; MAX_REELS]; MAX_SYMBOLS],
}
impl Default for RunRules {
    fn default() -> Self { Self { spans: [Span::default(); MAX_LINES], pays: [[0; MAX_REELS]; MAX_SYMBOLS] } }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct RunWin {
    pub start: u8,
    pub count: u8,
    pub symbol: u8,
    pub multiplier: u16,
}

impl RunRules {
    pub fn check(&self, card: &MachineConfig) -> Result<(), BadMachine> {
        card.check()?;
        for symbol in &self.pays { if symbol[0] != 0 || symbol[1] != 0 { return Err(BadMachine); } }
        for (i, span) in self.spans[..card.lines().len()].iter().enumerate() {
            if span.count < 3 || span.start as usize + span.count as usize > card.reels() { return Err(BadMachine); }
            for j in 0..i {
                let other=self.spans[j];
                // Overlapping collinear spans would pay the same run twice.
                let start=span.start.max(other.start) as usize;
                let end=(span.start+span.count).min(other.start+other.count) as usize;
                if end >= start+3 && (start..end).all(|r|card.lines[i].rows[r]==card.lines[j].rows[r]) { return Err(BadMachine); }
            }
        }
        Ok(())
    }
}

/// Each maximal run pays once; crossing paylines remain independent wins.
pub fn value_runs(card: &MachineConfig, rules: &RunRules, stops: &Stops) -> Result<(Winnings, [RunWin; MAX_LINES]), BadMachine> {
    rules.check(card)?;
    let grid=grid(card,stops);
    let mut wins=[RunWin::default();MAX_LINES];
    let mut total=Winnings::default();
    for (i,line) in card.lines().iter().enumerate() {
        let span=rules.spans[i];let end=(span.start+span.count) as usize;let mut start=span.start as usize;
        while start<end {
            let symbol=grid[start][line.rows[start] as usize];
            let mut next=start+1;
            while next<end && grid[next][line.rows[next] as usize]==symbol { next+=1; }
            let count=next-start;
            let multiplier=rules.pays[symbol as usize][count-1];
            if multiplier>0 {
                wins[i]=RunWin{start:start as u8,count:count as u8,symbol,multiplier};
                total.lines|=1<<i;
                let payout=((card.stake_lamports as u128 * multiplier as u128)/card.line_count as u128).min(u64::MAX as u128) as u64;
                total.lamports=total.lamports.saturating_add(payout);
            }
            start=next;
        }
    }
    Ok((total,wins))
}
