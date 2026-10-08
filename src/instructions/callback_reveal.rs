use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::observe;
use casino_core::{pda, vrf, CoreError};

use crate::state::spin::{Spin, SpinStatus};

/// The VRF oracle's answer: 32 bytes of randomness signed by the VRF identity scoped to this
/// program, written onto the spin as this round's pending seed. Applying it — turning it into
/// reels, a flip, a payout — is the round instruction's job, not this one's.
///
/// The seed is also the floor's result. If a station watches this bet, `RequestReveal` named the
/// house, the Magic context and the Magic program after the spin, and with them the house
/// schedules the floor's publish. A callback without them, requested before a station sat down
/// or before this program knew how, just lands the seed.
/// Accounts: [vrf_identity (signer), spin, house?, magic_context?, magic_program?]
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
        rest: &[AccountInfo],
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

        let Some(mut trailer) = Spin::observable(spin_account) else { return Ok(()) };
        trailer.status = observe::RESULT;
        trailer.round = spin.round;
        trailer.result[..32].copy_from_slice(&self.randomness);
        trailer.write(spin_account)?;

        if trailer.observer == [0; 32] {
            return Ok(());
        }
        let [house_account, magic_context, magic_program, ..] = rest else { return Ok(()) };
        // The accounts came from this program's own request, but the house still has to be the
        // house: it pays for the publish and signs for it.
        let (house, house_bump) = Pubkey::find_program_address(&[b"house"], program_id);
        if !observe::can_publish(&house, rest) {
            return Ok(());
        }
        observe::schedule_publish(
            program_id, house_account, &[b"house", &[house_bump]], magic_context, magic_program,
            spin_account, &trailer.observer, &spin.user,
        )
    }
}
