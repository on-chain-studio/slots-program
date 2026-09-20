use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError, pubkey::Pubkey};

use crate::constants::{is_admin, treasury_seed};
use crate::instruction::ProcessInstruction;
use crate::utils::{pda, vault};

/// Accounts: [admin (signer), treasury, ledger, reserve, permission, permission_program,
///            vault_program, token_program, system_program,
///            then (reserve_token, treasury_token) per non-zero mint, in entry order]
#[derive(BorshDeserialize)]
pub struct CloseLedger {
    pub which: u8,
}


impl ProcessInstruction for CloseLedger {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [admin, treasury, ledger, reserve, permission, permission_program,
             vault_program, token_program, system_program, extra @ ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        if !admin.is_signer || !is_admin(admin.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let seed = treasury_seed(self.which)?;
        let bump = pda::validate(program_id, treasury, &[seed])?;

        vault::close_ledger(
            vault_program, admin, treasury, ledger, reserve, permission, permission_program,
            token_program, system_program, extra,
            &[seed, &[bump]],
        )
    }
}
