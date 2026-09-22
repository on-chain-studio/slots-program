use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::AccountInfo, entrypoint::MAX_PERMITTED_DATA_INCREASE,
    entrypoint::ProgramResult, program::invoke, program_error::ProgramError,
};
use solana_system_interface::instruction as system_instruction;

use crate::constants::is_admin;
use crate::state::config::{Config, MACHINE_SIZE};
use crate::utils::pda;

/// Buys shelf room for more machines and pays the rent. Admin only. A big jump is several calls:
/// the per-instruction growth cap is checked here for a clear error.
/// Accounts: [admin (signer, payer), config, system_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct GrowConfig {
    pub add_machines: u16,
}

impl GrowConfig {
    #[inline(always)]
    pub fn process<'a>(
        &self,
        admin: &AccountInfo<'a>,
        config_account: &AccountInfo<'a>,
        system_program: &AccountInfo<'a>,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !admin.is_signer || !is_admin(admin.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        if self.add_machines == 0 {
            return Err(ProgramError::InvalidInstructionData);
        }
        pda::validate(program_id, config_account, &[b"config"])?;

        let old_len = config_account.data_len();
        let new_len = Config::size_for(Config::capacity(config_account) + self.add_machines as usize);
        let growth = new_len.saturating_sub(old_len);
        if growth == 0 || growth > MAX_PERMITTED_DATA_INCREASE {
            return Err(ProgramError::InvalidInstructionData);
        }

        // Rent first: a shelf grown below its exemption is collectable.
        let required = pda::min_balance(new_len);
        let held = config_account.lamports();
        if held < required {
            invoke(
                &system_instruction::transfer(admin.key, config_account.key, required - held),
                &[admin.clone(), config_account.clone(), system_program.clone()],
            )?;
        }

        // Fresh bytes are zeroed; `machine_count` still governs what's playable.
        config_account.resize(new_len)?;

        Ok(())
    }
}

/// How many machines fit in one growth step, for the client that has to chunk a big jump.
pub const MACHINES_PER_STEP: usize = MAX_PERMITTED_DATA_INCREASE / MACHINE_SIZE;
