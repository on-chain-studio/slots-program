use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};
use ephemeral_rollups_sdk::cpi::{delegate_account, DelegateAccounts, DelegateConfig, undelegate_account};
use ephemeral_rollups_sdk::ephem::{FoldableIntentBuilder, MagicIntentBundleBuilder};
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
        payer: &AccountInfo<'a>,
        pda: &AccountInfo<'a>,
        owner_program: &AccountInfo<'a>,
        buffer: &AccountInfo<'a>,
        delegation_record: &AccountInfo<'a>,
        delegation_metadata: &AccountInfo<'a>,
        delegation_program: &AccountInfo<'a>,
        system_program: &AccountInfo<'a>,
    ) -> ProgramResult {

        // Admin-gated: delegate_account zeroes the PDA's data on handoff, so open access is a griefing vector.
        if !payer.is_signer || !is_admin(payer.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let seeds: Vec<&[u8]> = self.pda_seeds.iter().map(|v| v.as_slice()).collect();

        delegate_account(
            DelegateAccounts {
                payer,
                pda,
                owner_program,
                buffer,
                delegation_record,
                delegation_metadata,
                delegation_program,
                system_program,
            },
            &seeds,
            DelegateConfig {
                commit_frequency_ms: 0,
                validator: Some(self.validator),
            },
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
        delegated_pda: &AccountInfo<'a>,
        buffer: &AccountInfo<'a>,
        payer: &AccountInfo<'a>,
        system_program: &AccountInfo<'a>,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        undelegate_account(
            delegated_pda,
            program_id,
            buffer,
            payer,
            system_program,
            self.pda_seeds.clone(),
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
        payer: &AccountInfo<'a>,
        pda: &AccountInfo<'a>,
        magic_context: &AccountInfo<'a>,
        magic_program: &AccountInfo<'a>,
        fees_vault: &AccountInfo<'a>,
    ) -> ProgramResult {
        // Admin-gated: otherwise anyone could undelegate the house and halt all settling.
        if !payer.is_signer || !is_admin(payer.key) {
            return Err(ProgramError::MissingRequiredSignature);
        }
        MagicIntentBundleBuilder::new(payer.clone(), magic_context.clone(), magic_program.clone())
            .magic_fee_vault(fees_vault.clone())
            .commit_and_undelegate(&[pda.clone()])
            .build_and_invoke()
    }
}
