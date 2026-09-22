use borsh::{BorshDeserialize, BorshSerialize};
use crate::chain::*;
use crate::magicblock::{commit_and_undelegate, delegate_account, undelegate_account};
use crate::constants::is_admin;

/// Delegates a PDA to a rollup validator named by the caller.
/// Accounts: payer, pda, owner_program, buffer, delegation_record, delegation_metadata, delegation_program, system_program
#[derive(BorshDeserialize, BorshSerialize)]
pub struct Delegate {
    pub pda_seeds: Vec<Vec<u8>>,
    pub validator: Pubkey,
}


impl Delegate {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        payer: &AccountInfo,
        pda: &AccountInfo,
        owner_program: &AccountInfo,
        buffer: &AccountInfo,
        delegation_record: &AccountInfo,
        delegation_metadata: &AccountInfo,
        delegation_program: &AccountInfo,
        system_program: &AccountInfo,
    ) -> ProgramResult {

        // Admin-gated: delegate_account zeroes the PDA's data on handoff, so open access is a griefing vector.
        if !payer.is_signer() || !is_admin(payer.address()) {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let seeds: Vec<&[u8]> = self.pda_seeds.iter().map(|v| v.as_slice()).collect();

        delegate_account(
            payer,
            pda,
            owner_program,
            buffer,
            delegation_record,
            delegation_metadata,
            delegation_program,
            system_program,
            &seeds,
            0,
            Some(self.validator),
        )
    }
}

/// Triggered by the delegation program via its fixed 8-byte discriminator.
/// Restores account ownership back to this program after the TEE session ends.
/// Accounts: delegated_pda, buffer, payer, system_program
#[derive(BorshDeserialize, BorshSerialize)]
pub struct Undelegate {
    pub pda_seeds: Vec<Vec<u8>>,
}


impl Undelegate {
    #[inline(always)]
    pub fn process<'a>(
        &self,
        delegated_pda: &AccountInfo,
        buffer: &AccountInfo,
        payer: &AccountInfo,
        system_program: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        undelegate_account(
            delegated_pda,
            program_id,
            buffer,
            payer,
            system_program,
            &self.pda_seeds,
        )
    }
}

/// Commits and undelegates a single PDA, returning it to L1. The admin is the fee payer, which is
/// also the only writable identity the rollup grants a non-delegated account.
/// Accounts: [payer, pda (writable), magic_context (writable), magic_program, fees_vault (writable)]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct RequestUndelegation;

impl RequestUndelegation {
    #[inline(always)]
    pub fn process<'a>(
        &self,
        payer: &AccountInfo,
        pda: &AccountInfo,
        magic_context: &AccountInfo,
        magic_program: &AccountInfo,
        fees_vault: &AccountInfo,
    ) -> ProgramResult {
        // Admin-gated: otherwise anyone could undelegate the house and halt all settling.
        if !payer.is_signer() || !is_admin(payer.address()) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        commit_and_undelegate(payer, magic_context, magic_program, Some(fees_vault), &[*pda])
    }
}
