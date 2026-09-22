use borsh::{BorshDeserialize, BorshSerialize};
use crate::chain::*;

use crate::constants::{is_admin, treasury_seed};
use crate::utils::{pda, vault};

/// Hands a treasury ledger to a rollup validator, named by the caller. basenet only, admin-gated.
/// Until it runs no card can settle: `settle` needs both ledgers on the same validator.
/// Accounts: [admin (signer), treasury, buffer, delegation_record, delegation_metadata,
///            ledger, permission, vault_program, delegation_program, system_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct DelegateTreasury {
    pub which: u8,
    pub validator: Pubkey,
}

impl DelegateTreasury {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        admin: &AccountInfo,
        treasury: &AccountInfo,
        buffer: &AccountInfo,
        delegation_record: &AccountInfo,
        delegation_metadata: &AccountInfo,
        ledger: &AccountInfo,
        vault_program: &AccountInfo,
        delegation_program: &AccountInfo,
        system_program: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !admin.is_signer() || !is_admin(admin.address()) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let seed = treasury_seed(self.which)?;
        let bump = pda::validate(program_id, treasury, &[seed])?;

        vault::delegate_ledger(
            vault_program, admin, treasury, buffer, delegation_record, delegation_metadata,
            ledger, delegation_program, system_program,
            &[seed, &[bump]],
            &self.validator,
        )
    }
}
