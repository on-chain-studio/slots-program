use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{pda, CoreError};

use crate::error::GameError;
use crate::instructions::pay_collect;
use crate::instructions::request_collect::{deciding, payout};
use crate::state::spin::{Spin, SpinStatus};

/// Asks the house to pay the finished spin: schedules its `PayCollect`, which pays it and marks
/// it collected. On a hold machine it also works mid-sequence, since collecting grid N equals
/// holding every reel and respinning, so stopping early forfeits nothing.
///
/// Names no ledger, so anyone's transaction or program may send it — the casino floor's house
/// among them. Who may ask is the only check: while a round decision is open, the player or the
/// session key that consented to the bet; once nothing is left to decide, anyone. A spin already
/// collected is left as it is.
///
/// Instruction 27, beside `RequestCollect` (22), the receipt-based collect every released client
/// sends: both price the spin by `payout`, both refuse a stranger by `deciding`; this one is what
/// the casino floor sends, and what clients move to.
/// Accounts: [collector (signer), user, house, spin, magic_context, magic_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct ScheduleCollect;

impl ScheduleCollect {
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
