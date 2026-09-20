use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, entrypoint::ProgramResult};

use crate::constants::{is_admin, treasury_seed};
use crate::instruction::ProcessInstruction;
use crate::utils::{pda, vault};

/// Brings a treasury ledger home from its rollup validator (mirror of `DelegateTreasury`). Rollup
/// only, admin-gated.
/// Accounts: [admin (signer), treasury, ledger, vault_program, magic_program, magic_context,
///            fees_vault]
#[derive(BorshDeserialize)]
pub struct UndelegateTreasury {
    pub which: u8,
}

impl ProcessInstruction for UndelegateTreasury {
    fn process(&self, program_id: &solana_program::pubkey::Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [admin, treasury, ledger, vault_program, magic_program, magic_context, fees_vault, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

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
