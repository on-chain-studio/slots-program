use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::instruction::ProcessInstruction;

/// A discriminator nothing implements — a retired variant, or one reserved for an instruction
/// not written yet. Kept so positions never shift.
#[derive(BorshDeserialize)]
pub struct Unknown;

impl ProcessInstruction for Unknown {
    fn process(&self, _program_id: &Pubkey, _accounts: &[AccountInfo]) -> ProgramResult {
        Err(ProgramError::InvalidInstructionData)
    }
}
