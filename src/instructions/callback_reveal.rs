use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{pda, vrf, CoreError};

use crate::state::spin::{Spin, SpinStatus};

/// The VRF oracle's answer: 32 bytes of randomness signed by the VRF identity scoped to this
/// program, written onto the spin as this round's pending seed. Applying it — turning it into
/// reels, a flip, a payout — is the round instruction's job, not this one's.
/// Accounts: [vrf_identity (signer), spin]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct CallbackReveal {
    pub randomness: [u8; 32],
    /// The round this seed was requested for, echoed from the request's callback args. A seed
    /// may only land in its own round: without this, a re-fired request's late duplicate could
    /// arrive after the player has seen round N and committed round N+1 — and if the oracle
    /// re-derives the same bytes, land a *known* value as the next round's randomness.
    pub round: u64,
    pub generation: u64,
}

impl CallbackReveal {
    #[inline(always)]
    pub fn process<'a>(
        &self,
        vrf_identity: &AccountInfo,
        spin_account: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !vrf_identity.is_signer() || *vrf_identity.address() != vrf::scoped_identity(program_id) {
            return Err(ProgramError::MissingRequiredSignature);
        }

        if Spin::generation(spin_account)? != self.generation {
            return Err(CoreError::WrongStatus.into());
        }
        let spin = Spin::load_mut(spin_account)?;
        if spin.status != SpinStatus::Requested as u64 {
            return Err(CoreError::WrongStatus.into());
        }
        if spin.round != self.round {
            return Err(CoreError::WrongStatus.into());
        }
        // The VRF identity signs whatever request named it; bind the callback to this user's spin.
        pda::validate(program_id, spin_account, &[b"spin", spin.user.as_ref()])?;
        spin.seed = self.randomness;
        spin.status = SpinStatus::Rolled as u64;

        Ok(())
    }
}
