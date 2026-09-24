use borsh::{BorshDeserialize, BorshSerialize};
use crate::magicblock::EPHEMERAL_VAULT_ID;
use crate::magicblock::create_ephemeral_account;
use crate::chain::*;

use crate::error::GameError;
use crate::state::analytics::Analytics;
use crate::state::spin::{self, Spin, SpinStatus};
use crate::state::Config;
use crate::utils::{pda, receipt};

/// Turns a settled stake into a spin. The seed comes later via `RequestReveal`, so a failed VRF
/// request cannot unwind a bet that is already paid for.
/// Accounts: [receipt, vault_authority (signer), config, house, spin, ephemeral_vault,
///            magic_program, analytics]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct ResolveBet {
    pub human: Pubkey,
    pub machine_id: u64,
    /// The session key that consented to the receipt — recorded so hold and gamble can accept
    /// its signature. The vault checked it against the user's ledger before settling, which is
    /// the only authorisation this program could not do itself.
    pub consenter: Pubkey,
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
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if *ephemeral_vault.address() != EPHEMERAL_VAULT_ID {
            return Err(GameError::InvalidPDA.into());
        }
        pda::validate(program_id, config_account, &[b"config"])?;
        let house_bump = pda::validate(program_id, house, &[b"house"])?;

        receipt::require_callback(vault_authority)?;

        let spin_bump = pda::validate(
            program_id, spin_account, &[b"spin", self.human.as_ref()],
        )?;
        // The account's existence is the one-bet-at-a-time mutex: a second stake settling while a
        // spin is live fails here — and having failed, the vault's post-CPI assertion unwinds the
        // whole settle, so the player is not charged for a spin that was never created.
        if spin_account.data_len() != 0 {
            return Err(GameError::AlreadyInitialized.into());
        }

        let terms = *Config::machine(config_account, self.machine_id)?;

        create_ephemeral_account(
            house,
            spin_account,
            ephemeral_vault,
            magic_program,
            Spin::WITH_TERMS as u32,
            &[
                &[b"house", &[house_bump]],
                &[b"spin", self.human.as_ref(), &[spin_bump]],
            ],
        )?;

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
        }
        Spin::write_terms(spin_account, &terms)?;

        // Make the spin private on the TEE. A stranger can otherwise derive ["spin", user] and read
        // the account and its entire signature history (every bet/reveal/collect, timestamped).
        // Members: the player's wallet, whose own TEE token authorises the client's reads and
        // subscriptions, and every program that is ever top-level over the spin in an ordinary
        // transaction — a private-rollup account admits one only when its top-level program is a
        // member: the vault (settle callbacks). The VRF oracle's callback is admitted without
        // membership, like a crank, so the VRF program is not on the list. ER-only (house fronts the
        // rent), never closed (closing would re-expose the not-yet-compressed history) and never
        // rewritten: an update through the ACL program drops the owning program from the list
        // and the rollup then refuses it for good. A permission is made once and left alone.
        if spin_permission.data_len() == 0 {
            let members = [self.human, crate::constants::VAULT_PROGRAM];
            let signers: &[&[&[u8]]] = &[
                &[b"house", &[house_bump]],
                &[b"spin", self.human.as_ref(), &[spin_bump]],
            ];
            crate::magicblock::create_ephemeral_permission(
                house, spin_account, spin_permission, ephemeral_vault, magic_program, permission_program, &members, signers,
            )?;
        }

        // This callback only fires on a settled stake, so the count is settled money.
        pda::validate(program_id, analytics_account, &[b"analytics"])?;
        let a = Analytics::load_mut(analytics_account)?;
        a.lamports_in = a.lamports_in.saturating_add(terms.stake_lamports);
        if let Some(slot) = a.bets_placed.get_mut(self.machine_id as usize) {
            Analytics::count(slot);
        }

        Ok(())
    }
}
