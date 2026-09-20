use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError, pubkey::Pubkey};

use crate::constants::{treasury_seed, is_admin};
use crate::instruction::ProcessInstruction;
use crate::utils::{pda, vault};

/// Flips a treasury ledger's privacy. Admin only, home-only (the vault refuses a delegated one).
/// Accounts: [admin (signer), treasury, ledger, permission, permission_program,
///            vault_program, system_program]
#[derive(BorshDeserialize)]
pub struct SetPrivacy {
    pub which: u8,
    /// 1 deletes the permission; 0 recreates it with the standard members.
    pub public: u8,
}


impl ProcessInstruction for SetPrivacy {
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

        vault::set_privacy(
            program_id,
            vault_program, admin, treasury, ledger, permission, permission_program,
            system_program,
            &[seed, &[bump]],
            self.public == 1,
        )
    }
}
