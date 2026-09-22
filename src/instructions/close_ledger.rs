use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError};

use crate::constants::{is_admin, treasury_seed};
use crate::utils::{pda, vault};

/// Accounts: [admin (signer), treasury, ledger, reserve, permission, permission_program,
///            vault_program, token_program, system_program,
///            then (reserve_token, treasury_token) per non-zero mint, in entry order]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct CloseLedger {
    pub which: u8,
}


impl CloseLedger {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        admin: &AccountInfo<'a>,
        treasury: &AccountInfo<'a>,
        ledger: &AccountInfo<'a>,
        reserve: &AccountInfo<'a>,
        permission: &AccountInfo<'a>,
        permission_program: &AccountInfo<'a>,
        vault_program: &AccountInfo<'a>,
        token_program: &AccountInfo<'a>,
        system_program: &AccountInfo<'a>,
        extra: &[AccountInfo<'a>],
    ) -> ProgramResult {
        let program_id = &crate::ID;

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
