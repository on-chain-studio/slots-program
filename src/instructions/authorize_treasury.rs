use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError};

use crate::constants::{is_admin, treasury_seed};
use crate::utils::{pda, vault};

/// One-time migration: sets a treasury ledger's member program so `settle_receipt` accepts it.
/// ER only, admin-gated, temporary — removed once both treasuries are migrated.
/// Accounts: [admin (signer), treasury, ledger, vault_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct AuthorizeTreasury {
    pub which: u8,
}

impl AuthorizeTreasury {
    #[inline(always)]
    pub fn process<'a>(
        &self,
        admin: &AccountInfo<'a>,
        treasury: &AccountInfo<'a>,
        ledger: &AccountInfo<'a>,
        vault_program: &AccountInfo<'a>,
    ) -> ProgramResult {
        let program_id = &crate::ID;
        if !admin.is_signer || !is_admin(admin.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let seed = treasury_seed(self.which)?;
        let bump = pda::validate(program_id, treasury, &[seed])?;

        vault::authorize_ledger(program_id, vault_program, treasury, ledger, &[seed, &[bump]])
    }
}
