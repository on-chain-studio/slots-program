use borsh::BorshDeserialize;
use ephemeral_rollups_sdk::consts::EPHEMERAL_VAULT_ID;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::constants::is_admin;
use crate::error::GameError;
use crate::instruction::ProcessInstruction;
use crate::utils::{pda, receipt};

/// Drops a spin and returns its rent to the house. Admin only — a spin is paid-for, so closing
/// one at will would destroy a player's bet; this is the escape hatch for a stranded one.
/// Accounts: [admin (signer), house, spin, ephemeral_vault, magic_program]
#[derive(BorshDeserialize)]
pub struct CloseSpin {
    pub user: Pubkey,
}

impl ProcessInstruction for CloseSpin {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [admin, house, spin_account, ephemeral_vault, magic_program, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        if !admin.is_signer || !is_admin(admin.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        if *ephemeral_vault.key != EPHEMERAL_VAULT_ID {
            return Err(GameError::InvalidPDA.into());
        }
        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        pda::validate(program_id, spin_account, &[b"spin", self.user.as_ref()])?;

        receipt::close(magic_program, house, spin_account, ephemeral_vault, house_bump)
    }
}
