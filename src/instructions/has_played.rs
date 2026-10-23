use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{pda, CoreError};

use crate::constants::QUEST_VERIFIER;
use crate::error::GameError;

/// Whether `wallet` has ever spun, for MagicBlock Quests. Answers the quest verifier key alone, and
/// only by simulation: a spin is private to its player on the TEE, so nobody else may learn even
/// that it exists. Every other caller gets the same `Unauthorized`, refused before the spin is
/// looked at, whether it is there or not.
///
/// The answer is success ("has played") or `NotPlayed`, never return data: the TEE runs a
/// simulation that names an account the caller may not read, then blanks its logs, compute units
/// and return data — but not its result. Every other failure is its own error, so a malformed
/// request never reads as "has not played".
/// Accounts: [verifier (signer), spin]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct HasPlayed {
    pub wallet: Pubkey,
}

impl HasPlayed {
    #[inline(always)]
    pub fn process(&self, verifier: &AccountInfo, spin: &AccountInfo) -> ProgramResult {
        if !verifier.is_signer() {
            return Err(ProgramError::MissingRequiredSignature);
        }
        if *verifier.address() != QUEST_VERIFIER {
            return Err(CoreError::Unauthorized.into());
        }
        pda::validate(&crate::ID, spin, &[b"spin", self.wallet.as_ref()])?;
        if spin.data_len() > 0 && spin.owned_by(&crate::ID) {
            Ok(())
        } else {
            Err(GameError::NotPlayed.into())
        }
    }
}
