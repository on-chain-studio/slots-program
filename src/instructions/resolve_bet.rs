use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::magicblock::{create_ephemeral_account, resize_ephemeral_account, EPHEMERAL_VAULT_ID, MEMBER_READ};
use casino_core::observe::{self, Observable};
use casino_core::permission;
use casino_core::ids::MAGIC_CONTEXT_ID;
use casino_core::referral::{self, Referral, Referrer};
use casino_core::{pda, receipt, CoreError};

use crate::constants::{ANALYTICS, PRIVATE_CASINO};
use crate::state::analytics::Analytics;
use crate::state::spin::{self, Spin, SpinStatus};
use crate::state::Config;

/// Turns a settled stake into a spin. The seed comes later via `RequestReveal`, so a failed VRF
/// request cannot unwind a bet that is already paid for.
///
/// A player seated at a casino floor station names it as one more account after these; it
/// becomes the spin's observer for this bet alone, so the reveal can tell the floor the result.
/// A bet without one clears it.
///
/// A linked player's settle, or one that names a referral code, also names the magic context: the
/// stake schedules its referrer's credit, or the link, from here, so the client never names the
/// referrer's account.
/// Accounts: [receipt, vault_authority (signer), config, house, spin, ephemeral_vault,
///            magic_program, analytics, spin_permission, permission_program, station?,
///            magic_context?]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct ResolveBet {
    pub human: Pubkey,
    pub machine_id: u64,
    /// The session key that consented to the receipt — recorded so hold and gamble can accept
    /// its signature. The vault checked it against the user's ledger before settling, which is
    /// the only authorisation this program could not do itself.
    pub consenter: Pubkey,
    /// The referral code the bet named, if any: read only while the spin is not linked.
    pub referrer: Referrer,
}

impl ResolveBet {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        _receipt_account: &AccountInfo,
        vault_authority: &AccountInfo,
        config_account: &AccountInfo,
        house: &AccountInfo,
        spin_account: &AccountInfo,
        ephemeral_vault: &AccountInfo,
        magic_program: &AccountInfo,
        analytics_account: &AccountInfo,
        spin_permission: &AccountInfo,
        permission_program: &AccountInfo,
        rest: &[AccountInfo],
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if *ephemeral_vault.address() != EPHEMERAL_VAULT_ID {
            return Err(CoreError::InvalidPDA.into());
        }
        pda::validate(program_id, config_account, &[b"config"])?;
        let house_bump = pda::validate(program_id, house, &[b"house"])?;

        receipt::require_callback(vault_authority)?;

        let spin_bump = pda::validate(
            program_id, spin_account, &[b"spin", self.human.as_ref()],
        )?;
        let kept = Spin::referral(spin_account);
        let (generation, previous_seed) = if spin_account.data_len() == 0 {
            (1, [0; 32])
        } else {
            if !spin_account.owned_by(program_id) { return Err(ProgramError::IllegalOwner); }
            let previous = *Spin::load(spin_account)?;
            if previous.user != self.human.to_bytes() || previous.discriminator != spin::DISCRIMINATOR {
                return Err(CoreError::Unauthorized.into());
            }
            if previous.status != SpinStatus::Collected as u64 { return Err(CoreError::AlreadyInitialized.into()); }
            (Spin::generation(spin_account)?.checked_add(1).ok_or(ProgramError::ArithmeticOverflow)?, previous.seed)
        };

        let terms = *Config::item(config_account, self.machine_id)?;

        // What the floor was last shown stays on show until this bet's seed lands over it; a
        // spin from before the trailer, or from its first layout, shows its last seed.
        let shown = match Spin::observable(spin_account) {
            Some(trailer) => trailer.result,
            None => {
                let mut result = [0; 64];
                result[..32].copy_from_slice(&previous_seed);
                result
            }
        };

        // Terms from before machines carried run rules are another shape: that spin is closed
        // and created again. Anything since only grew at the end — the generation, then the
        // trailer, then the trailer's second layout — so it grows in place, and its address and
        // the permission keyed to it stay. The trailer is written only once the spin is at its
        // full size, always over the last bytes, never over the terms or the generation. The
        // house sponsors the difference, as it did the rent.
        if spin_account.data_len() != 0 && spin_account.data_len() < Spin::WITH_TERMS {
            receipt::close(magic_program, house, spin_account, ephemeral_vault, house_bump)?;
        }
        if spin_account.data_len() == 0 {
            create_ephemeral_account(house, spin_account, ephemeral_vault, magic_program,
                Spin::OBSERVED_SIZE as u32,
                &[&[b"house", &[house_bump]], &[b"spin", self.human.as_ref(), &[spin_bump]]])?;
        } else if spin_account.data_len() != Spin::OBSERVED_SIZE {
            resize_ephemeral_account(house, spin_account, ephemeral_vault, magic_program,
                Spin::OBSERVED_SIZE as u32, &[&[b"house", &[house_bump]]])?;
        }
        Spin::set_generation(spin_account, generation)?;

        // A spin starts unlinked, whether new or grown from before referrals; only a credit links it.
        let section = kept.unwrap_or_else(Referral::unlinked);
        Spin::write_referral(spin_account, &section)?;

        {
            let s = Spin::load_mut(spin_account)?;
            s.discriminator = spin::DISCRIMINATOR;
            s.version = spin::VERSION;
            s.user = self.human.to_bytes();
            s.consenter = self.consenter.to_bytes();
            s.machine_id = self.machine_id;
            s.status = SpinStatus::Bought as u64;
            s.round = 0;
            s.hold = 0;
            s.pending = 0;
            s.stops = [0; 8];
            s.seed = previous_seed;
        }
        Spin::write_terms(spin_account, &terms)?;
        // The machine's stake is the whole bet: a slot takes nothing more once it is paid for, and
        // nothing is paid back until a seed lands.
        Observable {
            status: observe::PENDING,
            generation,
            round: 0,
            config_id: self.machine_id,
            stake: terms.stake_lamports,
            paid: 0,
            observer: observe::observer_from(rest),
            result: shown,
            ..Observable::new(observe::KIND_SLOTS)
        }
        .write(spin_account)?;

        permission::upgrade_ephemeral(
            program_id, permission_program, spin_account, &[b"spin", self.human.as_ref(), &[spin_bump]],
            spin_permission, house, &[b"house", &[house_bump]], ephemeral_vault, magic_program,
            spin_members(&self.human), MEMBER_READ,
        )?;

        // This callback only fires on a settled stake, so the count is settled money.
        pda::validate(program_id, analytics_account, &[b"analytics"])?;
        let a = Analytics::load_mut(analytics_account)?;
        a.lamports_in = a.lamports_in.saturating_add(terms.stake_lamports);
        if let Some(slot) = a.bets_placed.get_mut(self.machine_id as usize) {
            casino_core::analytics::count(slot);
        }
        if kept.is_none() {
            referral::count_player(Analytics::referrals(analytics_account));
        }

        // The referrer's credit is owed in lamports, so only a SOL stake earns one; a code still
        // links at no cut.
        let amount = if terms.mint == [0; 32] { referral::cut(terms.stake_lamports) } else { 0 };
        let house_seeds: &[&[u8]] = &[b"house", &[house_bump]];
        let magic_context = || rest.iter().find(|a| *a.address() == MAGIC_CONTEXT_ID).ok_or(ProgramError::NotEnoughAccountKeys);
        match (section.referrer(), self.referrer.0) {
            (Some(account), _) if amount > 0 => referral::schedule_credit(
                program_id, house, house_seeds, magic_context()?, magic_program,
                spin_account.address(), generation, &account, &ANALYTICS,
                crate::SlotsInstruction::CREDIT_REFERRAL, amount, false,
            )?,
            (None, Some(code)) => referral::schedule_link(
                program_id, house, house_seeds, magic_context()?, magic_program,
                &Pubkey::new_from_array(code), spin_account.address(), generation,
                crate::SlotsInstruction::LINK_REFERRAL, amount,
            )?,
            _ => {}
        }

        Ok(())
    }
}

pub fn spin_members(human: &Pubkey) -> Vec<Pubkey> {
    permission::ephemeral_members(&[*human, PRIVATE_CASINO])
}
