use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{account_info::AccountInfo, program_error::ProgramError, entrypoint::ProgramResult};

use crate::constants::{is_admin, treasury_seed};
use crate::utils::{pda, vault};

/// Brings a treasury ledger home from its rollup validator (mirror of `DelegateTreasury`). Rollup
/// only, admin-gated.
/// Accounts: [admin (signer), treasury, ledger, vault_program, magic_program, magic_context,
///            fees_vault]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct UndelegateTreasury {
    pub which: u8,
}

impl UndelegateTreasury {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        admin: &AccountInfo<'a>,
        treasury: &AccountInfo<'a>,
        ledger: &AccountInfo<'a>,
        vault_program: &AccountInfo<'a>,
        magic_program: &AccountInfo<'a>,
        magic_context: &AccountInfo<'a>,
        fees_vault: &AccountInfo<'a>,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !admin.is_signer || !is_admin(admin.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let seed = treasury_seed(self.which)?;
        let bump = pda::validate(program_id, treasury, &[seed])?;

        vault::undelegate_ledger(
            vault_program, admin, treasury, ledger, magic_program, magic_context, fees_vault,
            &[seed, &[bump]],
        )
    }
}
