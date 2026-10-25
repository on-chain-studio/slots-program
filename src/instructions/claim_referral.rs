use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::ids::VAULT_PROGRAM;
use casino_core::{pda, referral, vault, CoreError};

use crate::constants::ANALYTICS;
use crate::error::GameError;
use crate::state::spin::Spin;

/// Asks the house to pay a referrer what they are owed: schedules its `PayReferral`, which settles
/// it into the referrer's ledger. Names no ledger, so anyone may send it; the money only ever goes
/// to the spin's own player.
///
/// The referrer's ledger rides along to be looked at, not written: closed, or owned by the
/// delegation program (delegated to another rollup), the payout could not land, so the claim is
/// refused. On this rollup a ledger delegated here and one still at home both read as the vault's,
/// so the client checks basenet before claiming, for the one case this cannot see.
/// Accounts: [claimer (signer), user, house, spin, magic_context, magic_program, player_ledger]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct ClaimReferral;

impl ClaimReferral {
    #[inline(always)]
    pub fn process(
        &self,
        claimer: &AccountInfo,
        user: &AccountInfo,
        house: &AccountInfo,
        spin_account: &AccountInfo,
        magic_context: &AccountInfo,
        magic_program: &AccountInfo,
        player_ledger: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;
        if !claimer.is_signer() {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        pda::validate(program_id, spin_account, &[b"spin", user.address().as_ref()])?;
        let section = Spin::referral(spin_account).ok_or(CoreError::NothingToCollect)?;
        if section.owed == 0 {
            return Err(CoreError::NothingToCollect.into());
        }
        if *player_ledger.address() != vault::ledger(user.address()) {
            return Err(CoreError::InvalidPDA.into());
        }
        if player_ledger.data_len() == 0 || !player_ledger.owned_by(&VAULT_PROGRAM) {
            return Err(GameError::LedgerElsewhere.into());
        }
        referral::schedule_pay(
            program_id, house, &[b"house", &[house_bump]], magic_context, magic_program,
            spin_account.address(), user.address(), &ANALYTICS, crate::SlotsInstruction::PAY_REFERRAL,
        )
    }
}
