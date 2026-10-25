use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{pda, referral, vault, CoreError};

use crate::constants::ANALYTICS;
use crate::state::analytics::Analytics;
use crate::state::spin::Spin;

/// The house paying a referrer what they are owed. Only its own task fires it: the executor
/// signs for the house that scheduled it. Nothing owed by the time it fires is nothing to do.
/// Accounts: [executor (signer), house, house_ledger, spin, player_ledger, vault_program, analytics]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct PayReferral;

impl PayReferral {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &self,
        executor: &AccountInfo,
        house: &AccountInfo,
        house_ledger: &AccountInfo,
        spin_account: &AccountInfo,
        player_ledger: &AccountInfo,
        vault_program: &AccountInfo,
        analytics_account: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;
        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        referral::require_executor(executor, house)?;
        if *house_ledger.address() != vault::ledger(house.address()) || *analytics_account.address() != ANALYTICS {
            return Err(CoreError::InvalidPDA.into());
        }
        let Some(mut section) = Spin::referral(spin_account) else { return Ok(()) };
        let user = Spin::load(spin_account)?.user;
        pda::validate(program_id, spin_account, &[b"spin", &user])?;
        if *player_ledger.address() != vault::ledger(&Pubkey::new_from_array(user)) {
            return Err(CoreError::InvalidPDA.into());
        }
        let owed = referral::take_owed(&mut section, Analytics::referrals(analytics_account));
        if owed == 0 {
            return Ok(());
        }
        Spin::write_referral(spin_account, &section)?;
        referral::pay(vault_program, house, house_ledger, player_ledger, &[b"house", &[house_bump]], owed)
    }
}
