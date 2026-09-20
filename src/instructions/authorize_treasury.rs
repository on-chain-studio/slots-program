use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError, pubkey::Pubkey};

use crate::constants::{is_admin, treasury_seed};
use crate::instruction::ProcessInstruction;
use crate::utils::{pda, vault};

/// One-time migration: sets a treasury ledger's member program so `settle_receipt` accepts it.
/// ER only, admin-gated, temporary — removed once both treasuries are migrated.
/// Accounts: [admin (signer), treasury, ledger, vault_program]
#[derive(BorshDeserialize)]
pub struct AuthorizeTreasury {
    pub which: u8,
}

impl ProcessInstruction for AuthorizeTreasury {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [admin, treasury, ledger, vault_program, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };
        if !admin.is_signer || !is_admin(admin.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let seed = treasury_seed(self.which)?;
        let bump = pda::validate(program_id, treasury, &[seed])?;

        vault::authorize_ledger(program_id, vault_program, treasury, ledger, &[seed, &[bump]])
    }
}
