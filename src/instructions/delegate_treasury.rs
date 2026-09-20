use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::constants::{is_admin, treasury_seed};
use crate::instruction::ProcessInstruction;
use crate::utils::{pda, vault};

/// Hands a treasury ledger to a rollup validator, named by the caller. basenet only, admin-gated.
/// Until it runs no card can settle: `settle` needs both ledgers on the same validator.
/// Accounts: [admin (signer), treasury, buffer, delegation_record, delegation_metadata,
///            ledger, permission, vault_program, delegation_program, system_program]
#[derive(BorshDeserialize)]
pub struct DelegateTreasury {
    pub which: u8,
    pub validator: Pubkey,
}

impl ProcessInstruction for DelegateTreasury {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [admin, treasury, buffer, delegation_record, delegation_metadata,
             ledger, vault_program, delegation_program, system_program, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        if !admin.is_signer || !is_admin(admin.key) {
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
