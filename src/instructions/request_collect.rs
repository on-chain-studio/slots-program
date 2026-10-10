use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{pda, CoreError};

use crate::error::GameError;
use crate::instructions::pay_collect;
use crate::state::config::{MODE_GAMBLE, MODE_HOLD};
use crate::state::spin::{Spin, SpinStatus};
use crate::utils::engine;

/// Asks the house to pay the finished spin: schedules its `PayCollect`, which pays it and marks
/// it collected. On a hold machine it also works mid-sequence, since collecting grid N equals
/// holding every reel and respinning, so stopping early forfeits nothing.
///
/// Names no ledger, so anyone's transaction or program may send it — the casino floor's house
/// among them. Who may ask is the only check: while a round decision is open, the player or the
/// session key that consented to the bet; once nothing is left to decide, anyone. A spin already
/// collected is left as it is.
/// Accounts: [collector (signer), user, house, spin, magic_context, magic_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct RequestCollect;

/// What a finished spin pays, given everything the account holds. Pure, and shared with
/// `pay_collect` and the reveal callback, so the number the floor shows, the number the vault
/// moves and the number analytics records are all the same computation.
pub fn payout(spin: &Spin, terms: &crate::state::MachineConfig) -> Result<u64, ProgramError> {
    match terms.mode {
        MODE_GAMBLE if spin.round > 0 => {
            // On the ladder: the pending seed is a flip over what rides.
            Ok(if engine::gamble(terms, &spin.seed)? {
                spin.pending.checked_mul(2).ok_or(ProgramError::ArithmeticOverflow)?
            } else {
                0
            })
        }
        MODE_HOLD => {
            let stops = engine::spin(terms, &spin.seed, spin.hold as u8, &spin.stops())?;
            Ok(engine::value(terms, &stops)?.lamports)
        }
        _ => {
            let stops = engine::spin(terms, &spin.seed, 0, &spin.stops())?;
            Ok(engine::value(terms, &stops)?.lamports)
        }
    }
}

/// Whether the rolled spin, paying `amount` if collected now, still has a round decision open: a
/// hold respin, or a gamble win that can ride further. Shared with `callback_reveal`, so the floor
/// is told a decision is open exactly when the collect would refuse anyone but the consenter.
pub fn deciding(spin: &Spin, terms: &crate::state::MachineConfig, amount: u64) -> bool {
    match terms.mode {
        MODE_HOLD => spin.round + 1 < terms.rounds() as u64,
        MODE_GAMBLE => amount > 0 && spin.round < terms.gamble_rungs as u64,
        _ => false,
    }
}

impl RequestCollect {
    #[inline(always)]
    pub fn process(
        &self,
        collector: &AccountInfo,
        user: &AccountInfo,
        house: &AccountInfo,
        spin_account: &AccountInfo,
        magic_context: &AccountInfo,
        magic_program: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;
        if !collector.is_signer() {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let house_bump = pda::validate(program_id, house, &[b"house"])?;

        let terms = *Spin::terms(spin_account)?;
        let (generation, round) = {
            let spin = Spin::load(spin_account)?;
            if spin.user != user.address().to_bytes() {
                return Err(CoreError::Unauthorized.into());
            }
            pda::validate(program_id, spin_account, &[b"spin", user.address().as_ref()])?;
            if spin.status == SpinStatus::Collected as u64 {
                return Ok(());
            }
            if spin.status != SpinStatus::Rolled as u64 {
                return Err(GameError::NotRolled.into());
            }
            // While a round decision is still open only the player may collect. Otherwise a
            // stranger could force-settle the current grid and rob the player of their remaining
            // respins or ladder climbs.
            let asker = collector.address().to_bytes();
            if deciding(spin, &terms, payout(spin, &terms)?) && asker != spin.user && asker != spin.consenter {
                return Err(CoreError::Unauthorized.into());
            }
            (Spin::generation(spin_account)?, spin.round)
        };

        pay_collect::schedule(
            house, &[b"house", &[house_bump]], magic_context, magic_program,
            spin_account.address(), user.address(), generation, round,
        )
    }
}
