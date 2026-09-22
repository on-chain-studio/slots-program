use borsh::{BorshDeserialize, BorshSerialize};
use crate::chain::*;

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
        admin: &AccountInfo,
        treasury: &AccountInfo,
        ledger: &AccountInfo,
        vault_program: &AccountInfo,
        magic_program: &AccountInfo,
        magic_context: &AccountInfo,
        fees_vault: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !admin.is_signer() || !is_admin(admin.address()) {
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
