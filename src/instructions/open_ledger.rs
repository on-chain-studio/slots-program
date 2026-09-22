use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError};

use crate::constants::{is_admin, treasury_seed};
use crate::utils::{pda, vault};

/// Accounts: [admin (signer), treasury, ledger, permission, permission_program,
///            vault_program, system_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct OpenLedger {
    pub which: u8,
    pub slots: u16,
}


impl OpenLedger {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        admin: &AccountInfo<'a>,
        treasury: &AccountInfo<'a>,
        ledger: &AccountInfo<'a>,
        permission: &AccountInfo<'a>,
        permission_program: &AccountInfo<'a>,
        vault_program: &AccountInfo<'a>,
        system_program: &AccountInfo<'a>,
    ) -> ProgramResult {
        let program_id = &crate::ID;

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
