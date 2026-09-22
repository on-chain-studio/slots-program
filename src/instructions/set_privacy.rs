use borsh::{BorshDeserialize, BorshSerialize};
use crate::chain::*;

use crate::constants::{treasury_seed, is_admin};
use crate::utils::{pda, vault};

/// Flips a treasury ledger's privacy. Admin only, home-only (the vault refuses a delegated one).
/// Accounts: [admin (signer), treasury, ledger, permission, permission_program,
///            vault_program, system_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct SetPrivacy {
    pub which: u8,
    /// 1 deletes the permission; 0 recreates it with the standard members.
    pub public: u8,
}


impl SetPrivacy {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        admin: &AccountInfo,
        treasury: &AccountInfo,
        ledger: &AccountInfo,
        permission: &AccountInfo,
        permission_program: &AccountInfo,
        vault_program: &AccountInfo,
        system_program: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !admin.is_signer() || !is_admin(admin.address()) {
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
