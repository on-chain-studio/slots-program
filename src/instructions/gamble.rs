use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{observe, pda, CoreError};

use crate::error::GameError;
use crate::state::config::MODE_GAMBLE;
use crate::state::spin::{Spin, SpinStatus};
use crate::utils::engine;

/// Applies the seen result and commits the win to one more rung of the ladder. Signed by the
/// player or the recorded consenter. Same ordering rule as `Hold`: the decision to risk it is on
/// chain before the flip's randomness can exist.
///
/// Round 0 applies the base spin — there must be a win to risk. Later rounds apply the previous
/// flip: a won flip doubles what rides, a lost one leaves nothing to climb with, and the way off
/// the ladder at any point — banked, busted, or done — is `RequestCollect`.
/// Accounts: [signer, spin]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct Gamble;

impl Gamble {
    #[inline(always)]
    pub fn process<'a>(
        &self,
        signer: &AccountInfo,
        spin_account: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !signer.is_signer() {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let terms = *Spin::terms(spin_account)?;
        let spin = Spin::load_mut(spin_account)?;

        let key = signer.address().to_bytes();
        if key != spin.user && key != spin.consenter {
            return Err(CoreError::Unauthorized.into());
        }
        pda::validate(program_id, spin_account, &[b"spin", spin.user.as_ref()])?;

        if terms.mode != MODE_GAMBLE {
            return Err(CoreError::WrongStatus.into());
        }
        if spin.status != SpinStatus::Rolled as u64 {
            return Err(GameError::NotRolled.into());
        }
        // After this commit the next seed is rung `round + 1`; the ladder has `gamble_rungs`.
        if spin.round >= terms.gamble_rungs as u64 {
            return Err(GameError::RoundsExhausted.into());
        }

        if spin.round == 0 {
            let stops = engine::spin(&terms, &spin.seed, 0, &spin.stops())?;
            let w = engine::value(&terms, &stops)?;
            if w.lamports == 0 {
                return Err(CoreError::NothingToCollect.into());
            }
            spin.set_stops(&stops);
            spin.pending = w.lamports;
        } else {
            if engine::gamble(&terms, &spin.seed)? {
                spin.pending = spin
                    .pending
                    .checked_mul(2)
                    .ok_or(ProgramError::ArithmeticOverflow)?;
            } else {
                // Busted. Nothing rides, so there is nothing to put at risk again.
                return Err(CoreError::NothingToCollect.into());
            }
        }

        spin.seed = [0u8; 32];
        spin.round += 1;
        spin.status = SpinStatus::Bought as u64;

        // Waiting on a seed again, for the same bet and whoever watches it.
        Spin::observe(spin_account, observe::PENDING)
    }
}
