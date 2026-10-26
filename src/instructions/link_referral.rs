use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{pda, referral};

use crate::constants::ANALYTICS;

/// Finds the referrer behind a referral code: the house's task, scheduled when an unlinked
/// player's stake settled naming one. The code is the referrer's player account in any game;
/// its owner's spin here gets the credit that links the player, if that spin exists.
/// Accounts: [executor (signer), house, code, spin (the player's), magic_context, magic_program]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct LinkReferral {
    pub amount: u64,
    pub generation: u64,
}

impl LinkReferral {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &self,
        executor: &AccountInfo,
        house: &AccountInfo,
        code: &AccountInfo,
        spin_account: &AccountInfo,
        magic_context: &AccountInfo,
        magic_program: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;
        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        referral::link(
            program_id, executor, house, &[b"house", &[house_bump]], code, spin_account,
            magic_context, magic_program, b"spin", &ANALYTICS,
            crate::SlotsInstruction::CREDIT_REFERRAL, self.amount, self.generation,
        )
    }
}
