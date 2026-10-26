use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::magicblock::{resize_ephemeral_account, EPHEMERAL_VAULT_ID};
use casino_core::observe;
use casino_core::referral::{self, Referral};
use casino_core::{pda, CoreError};
use pinocchio::sysvars::{clock::Clock, Sysvar};

use crate::constants::ANALYTICS;
use crate::state::analytics::Analytics;
use crate::state::spin::{Spin, SpinStatus};

/// Credits a referrer their cut of a stake: the house's task, scheduled when a referred player's
/// stake settled. The amount is the task's own, and only the house's executor signs one.
///
/// A referrer's spin of any earlier layout is brought to the current one here, unlinked, so no
/// credit waits on their next bet: the layouts that only grew at the end grow in place, the floor's
/// trailer moving with the end; the older ones are rewritten once their bet is collected, keeping
/// the generation. A spin gone, or an old one still in play, is left alone: the house keeps that cut.
///
/// A credit from a link names the player's spin after these, and links it to the referrer's spin
/// only once that can hold its credits; a player linked to someone else since is left alone, and so
/// is the cut.
/// Accounts: [executor (signer), house, spin (the referrer's), analytics, ephemeral_vault,
///            magic_program, player's spin?]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct CreditReferral {
    pub amount: u64,
}

impl CreditReferral {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &self,
        executor: &AccountInfo,
        house: &AccountInfo,
        spin_account: &AccountInfo,
        analytics_account: &AccountInfo,
        ephemeral_vault: &AccountInfo,
        magic_program: &AccountInfo,
        rest: &[AccountInfo],
    ) -> ProgramResult {
        let program_id = &crate::ID;
        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        referral::require_executor(executor, house)?;
        if *analytics_account.address() != ANALYTICS || *ephemeral_vault.address() != EPHEMERAL_VAULT_ID {
            return Err(CoreError::InvalidPDA.into());
        }
        if spin_account.data_len() < Spin::SIZE || !spin_account.owned_by(program_id) {
            return Ok(());
        }
        let user = Spin::load(spin_account)?.user;
        pda::validate(program_id, spin_account, &[b"spin", &user])?;
        if Spin::referral(spin_account).is_none()
            && !bring_up(spin_account, house, house_bump, analytics_account, ephemeral_vault, magic_program)?
        {
            return Ok(());
        }

        if let Some(player) = rest.first() {
            if player.address() == spin_account.address() || !player.owned_by(program_id) {
                return Ok(());
            }
            let owner = Spin::load(player)?.user;
            pda::validate(program_id, player, &[b"spin", &owner])?;
            let Some(mut linked) = Spin::referral(player) else { return Ok(()) };
            if !linked.link(spin_account.address(), Clock::get()?.unix_timestamp, Analytics::referrals(analytics_account)) {
                return Ok(());
            }
            Spin::write_referral(player, &linked)?;
            if self.amount == 0 {
                return Ok(());
            }
        }

        let mut section = Spin::referral(spin_account).ok_or(ProgramError::InvalidAccountData)?;
        referral::credit(&mut section, Analytics::referrals(analytics_account), self.amount);
        Spin::write_referral(spin_account, &section)
    }
}

/// Brings a spin from before referrals to the current layout, unlinked, and counts its player.
/// False for an old layout whose bet is still in play, which only its own next bet may change.
fn bring_up(
    spin_account: &AccountInfo,
    house: &AccountInfo,
    house_bump: u8,
    analytics_account: &AccountInfo,
    ephemeral_vault: &AccountInfo,
    magic_program: &AccountInfo,
) -> Result<bool, ProgramError> {
    let len = spin_account.data_len();
    let in_place = [Spin::PERSISTENT_SIZE, Spin::FIRST_OBSERVED_SIZE, Spin::PRE_REFERRAL_SIZE].contains(&len);
    if !in_place && Spin::load(spin_account)?.status != SpinStatus::Collected as u64 {
        return Ok(false);
    }
    let generation = (!in_place).then(|| Spin::generation(spin_account).unwrap_or(0));
    let trailer = (len == Spin::PRE_REFERRAL_SIZE)
        .then(|| spin_account.try_borrow().map(|d| d[len - observe::SIZE..].to_vec()))
        .transpose()?;
    resize_ephemeral_account(house, spin_account, ephemeral_vault, magic_program,
        Spin::OBSERVED_SIZE as u32, &[&[b"house", &[house_bump]]])?;
    {
        // An old layout's terms are of another shape; its bet is collected, and its next bet
        // prints the machine's terms afresh.
        let mut data = spin_account.try_borrow_mut_data()?;
        let tail = &mut data[if in_place { Spin::REFERRAL_AT } else { Spin::SIZE }..];
        tail.fill(0);
        if let Some(trailer) = trailer {
            let at = tail.len() - observe::SIZE;
            tail[at..].copy_from_slice(&trailer);
        }
    }
    if let Some(generation) = generation {
        Spin::set_generation(spin_account, generation)?;
    }
    Spin::write_referral(spin_account, &Referral::unlinked())?;
    referral::count_player(Analytics::referrals(analytics_account));
    Ok(true)
}
