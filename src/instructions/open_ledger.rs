use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError, pubkey::Pubkey};

use crate::constants::{is_admin, treasury_seed};
use crate::instruction::ProcessInstruction;
use crate::utils::{pda, vault};

/// Accounts: [admin (signer), treasury, ledger, permission, permission_program,
///            vault_program, system_program]
#[derive(BorshDeserialize)]
pub struct OpenLedger {
    pub which: u8,
    pub slots: u16,
}


impl ProcessInstruction for OpenLedger {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [admin, treasury, ledger, permission, permission_program,
             vault_program, system_program, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        if !admin.is_signer || !is_admin(admin.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let seed = treasury_seed(self.which)?;
        let bump = pda::validate(program_id, treasury, &[seed])?;

        vault::open_ledger(
            program_id,
            vault_program, admin, treasury, ledger, permission, permission_program, system_program,
            &[seed, &[bump]],
            self.slots,
        )
    }
}
