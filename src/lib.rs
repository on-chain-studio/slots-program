//! Slot Machines. The `#[program]` block below is the whole wire interface: every instruction, its
//! number, its accounts in order and its arguments. What each one does lives in `instructions`.

use solarium::prelude::*;
use solarium_program::prelude::*;
use solarium_program::{Account, Remaining, Signer};

use crate::instructions::*;

pub mod constants;
pub mod error;

pub mod instructions {
    pub mod initialize;
    pub mod set_machine;
    pub mod grow_config;
    pub mod close_spin;
    pub mod delegation;
    pub mod open_ledger;
    pub mod delegate_treasury;
    pub mod undelegate_treasury;
    pub mod close_ledger;
    pub mod authorize_treasury;
    pub mod set_privacy;
    pub mod withdraw_house;

    pub mod request_bet;
    pub mod resolve_bet;
    pub mod request_reveal;
    pub mod callback_reveal;
    pub mod hold;
    pub mod gamble;
    pub mod request_collect;
    pub mod resolve_collect;
}

pub mod state;

pub mod utils {
    pub mod pda;
    pub mod engine;
    pub mod vrf;
    pub mod vault;
    pub mod receipt;
}

// Each discriminator is the little-endian u64 an instruction starts with, and they are the ones
// this program has always had: dense, append-only, never reused — renumbering would silently
// repoint old clients. 0 and 15 are answered by nothing (a retired slot and a reserved one), and
// the settle and VRF callbacks are called back by these numbers, so they can no more move than
// the rest. Accounts are taken in the order listed; any past the last are ignored.
//
// `Signer` stands only where the handler's first check was that very signature, so a refusal
// still reads MissingRequiredSignature. The callbacks' authorities stay plain accounts: they are
// checked against a known key, and refused with the game's own error.
#[program(id = "SLoTSdnmBH5KtNJjhEYw1MeWTKAfRnFfQTTcpgwRn2Q")]
impl Slots {
    #[instruction(discriminator = 1)]
    pub fn initialize<'a>(
        &self,
        initializer: &Signer<'a>,
        config: &mut Account<'a>,
        house: &mut Account<'a>,
        analytics: &mut Account<'a>,
        permission: &mut Account<'a>,
        permission_program: &Account<'a>,
        system_program: &Account<'a>,
        args: initialize::Initialize,
    ) -> Result<()> {
        Ok(args.process(
            initializer.info, config.info, house.info, analytics.info, permission.info,
            permission_program.info, system_program.info,
        )?)
    }

    #[instruction(discriminator = 2)]
    pub fn delegate<'a>(
        &self,
        payer: &Signer<'a>,
        pda: &mut Account<'a>,
        owner_program: &Account<'a>,
        buffer: &mut Account<'a>,
        delegation_record: &mut Account<'a>,
        delegation_metadata: &mut Account<'a>,
        delegation_program: &Account<'a>,
        system_program: &Account<'a>,
        args: delegation::Delegate,
    ) -> Result<()> {
        Ok(args.process(
            payer.info, pda.info, owner_program.info, buffer.info, delegation_record.info,
            delegation_metadata.info, delegation_program.info, system_program.info,
        )?)
    }

    /// Called by the delegation program with its own fixed tag; 3 is kept as it always was.
    #[instruction(discriminator = 3, alias = "global:process_undelegation")]
    pub fn undelegate<'a>(
        &self,
        delegated_pda: &mut Account<'a>,
        buffer: &mut Account<'a>,
        payer: &mut Account<'a>,
        system_program: &Account<'a>,
        args: delegation::Undelegate,
    ) -> Result<()> {
        Ok(args.process(delegated_pda.info, buffer.info, payer.info, system_program.info)?)
    }

    #[instruction(discriminator = 4)]
    pub fn request_undelegation<'a>(
        &self,
        payer: &Signer<'a>,
        pda: &mut Account<'a>,
        magic_context: &mut Account<'a>,
        magic_program: &Account<'a>,
        fees_vault: &mut Account<'a>,
    ) -> Result<()> {
        Ok(delegation::RequestUndelegation.process(
            payer.info, pda.info, magic_context.info, magic_program.info, fees_vault.info,
        )?)
    }

    #[instruction(discriminator = 5)]
    pub fn set_machine<'a>(
        &self,
        admin: &Signer<'a>,
        config: &mut Account<'a>,
        args: set_machine::SetMachine,
    ) -> Result<()> {
        Ok(args.process(admin.info, config.info)?)
    }

    #[instruction(discriminator = 6)]
    pub fn grow_config<'a>(
        &self,
        admin: &Signer<'a>,
        config: &mut Account<'a>,
        system_program: &Account<'a>,
        args: grow_config::GrowConfig,
    ) -> Result<()> {
        Ok(args.process(admin.info, config.info, system_program.info)?)
    }

    #[instruction(discriminator = 7)]
    pub fn close_spin<'a>(
        &self,
        admin: &Signer<'a>,
        house: &mut Account<'a>,
        spin: &mut Account<'a>,
        ephemeral_vault: &mut Account<'a>,
        magic_program: &Account<'a>,
        args: close_spin::CloseSpin,
    ) -> Result<()> {
        Ok(args.process(
            admin.info, house.info, spin.info, ephemeral_vault.info, magic_program.info,
        )?)
    }

    #[instruction(discriminator = 8)]
    pub fn open_ledger<'a>(
        &self,
        admin: &Signer<'a>,
        treasury: &mut Account<'a>,
        ledger: &mut Account<'a>,
        permission: &mut Account<'a>,
        permission_program: &Account<'a>,
        vault_program: &Account<'a>,
        system_program: &Account<'a>,
        args: open_ledger::OpenLedger,
    ) -> Result<()> {
        Ok(args.process(
            admin.info, treasury.info, ledger.info, permission.info, permission_program.info,
            vault_program.info, system_program.info,
        )?)
    }

    #[instruction(discriminator = 9)]
    pub fn delegate_treasury<'a>(
        &self,
        admin: &Signer<'a>,
        treasury: &mut Account<'a>,
        buffer: &mut Account<'a>,
        delegation_record: &mut Account<'a>,
        delegation_metadata: &mut Account<'a>,
        ledger: &mut Account<'a>,
        vault_program: &Account<'a>,
        delegation_program: &Account<'a>,
        system_program: &Account<'a>,
        args: delegate_treasury::DelegateTreasury,
    ) -> Result<()> {
        Ok(args.process(
            admin.info, treasury.info, buffer.info, delegation_record.info,
            delegation_metadata.info, ledger.info, vault_program.info, delegation_program.info,
            system_program.info,
        )?)
    }

    #[instruction(discriminator = 10)]
    pub fn undelegate_treasury<'a>(
        &self,
        admin: &Signer<'a>,
        treasury: &mut Account<'a>,
        ledger: &mut Account<'a>,
        vault_program: &Account<'a>,
        magic_program: &Account<'a>,
        magic_context: &mut Account<'a>,
        fees_vault: &mut Account<'a>,
        args: undelegate_treasury::UndelegateTreasury,
    ) -> Result<()> {
        Ok(args.process(
            admin.info, treasury.info, ledger.info, vault_program.info, magic_program.info,
            magic_context.info, fees_vault.info,
        )?)
    }

    /// Takes a (reserve token, treasury token) pair per non-zero mint after the named accounts.
    #[instruction(discriminator = 11)]
    pub fn close_ledger<'a>(
        &self,
        admin: &Signer<'a>,
        treasury: &mut Account<'a>,
        ledger: &mut Account<'a>,
        reserve: &mut Account<'a>,
        permission: &mut Account<'a>,
        permission_program: &Account<'a>,
        vault_program: &Account<'a>,
        token_program: &Account<'a>,
        system_program: &Account<'a>,
        token_accounts: &Remaining<'a>,
        args: close_ledger::CloseLedger,
    ) -> Result<()> {
        Ok(args.process(
            admin.info, treasury.info, ledger.info, reserve.info, permission.info,
            permission_program.info, vault_program.info, token_program.info, system_program.info,
            token_accounts,
        )?)
    }

    #[instruction(discriminator = 12)]
    pub fn authorize_treasury<'a>(
        &self,
        admin: &Signer<'a>,
        treasury: &mut Account<'a>,
        ledger: &mut Account<'a>,
        vault_program: &Account<'a>,
        args: authorize_treasury::AuthorizeTreasury,
    ) -> Result<()> {
        Ok(args.process(admin.info, treasury.info, ledger.info, vault_program.info)?)
    }

    #[instruction(discriminator = 13)]
    pub fn set_privacy<'a>(
        &self,
        admin: &Signer<'a>,
        treasury: &mut Account<'a>,
        ledger: &mut Account<'a>,
        permission: &mut Account<'a>,
        permission_program: &Account<'a>,
        vault_program: &Account<'a>,
        system_program: &Account<'a>,
        args: set_privacy::SetPrivacy,
    ) -> Result<()> {
        Ok(args.process(
            admin.info, treasury.info, ledger.info, permission.info, permission_program.info,
            vault_program.info, system_program.info,
        )?)
    }

    #[instruction(discriminator = 14)]
    pub fn withdraw_house<'a>(
        &self,
        admin: &Signer<'a>,
        house: &mut Account<'a>,
        house_ledger: &mut Account<'a>,
        admin_ledger: &mut Account<'a>,
        vault_program: &Account<'a>,
        args: withdraw_house::WithdrawHouse,
    ) -> Result<()> {
        Ok(args.process(
            admin.info, house.info, house_ledger.info, admin_ledger.info, vault_program.info,
        )?)
    }

    #[instruction(discriminator = 16)]
    pub fn request_bet<'a>(
        &self,
        wallet: &Signer<'a>,
        user: &Account<'a>,
        config: &Account<'a>,
        house: &mut Account<'a>,
        receipt: &mut Account<'a>,
        ephemeral_vault: &mut Account<'a>,
        magic_program: &Account<'a>,
        vault_program: &Account<'a>,
        house_ledger: &mut Account<'a>,
        magic_context: &mut Account<'a>,
        args: request_bet::RequestBet,
    ) -> Result<()> {
        Ok(args.process(
            wallet.info, user.info, config.info, house.info, receipt.info, ephemeral_vault.info,
            magic_program.info, vault_program.info, house_ledger.info, magic_context.info,
        )?)
    }

    /// The vault's settle callback for a stake.
    #[instruction(discriminator = 17)]
    pub fn resolve_bet<'a>(
        &self,
        receipt: &Account<'a>,
        vault_authority: &Account<'a>,
        config: &Account<'a>,
        house: &mut Account<'a>,
        spin: &mut Account<'a>,
        ephemeral_vault: &mut Account<'a>,
        magic_program: &Account<'a>,
        analytics: &mut Account<'a>,
        args: resolve_bet::ResolveBet,
    ) -> Result<()> {
        Ok(args.process(
            receipt.info, vault_authority.info, config.info, house.info, spin.info,
            ephemeral_vault.info, magic_program.info, analytics.info,
        )?)
    }

    #[instruction(discriminator = 18)]
    pub fn request_reveal<'a>(
        &self,
        user: &Account<'a>,
        house: &mut Account<'a>,
        spin: &mut Account<'a>,
        identity: &Account<'a>,
        oracle_queue: &mut Account<'a>,
        slot_hashes: &Account<'a>,
        system_program: &Account<'a>,
        vrf_program: &Account<'a>,
    ) -> Result<()> {
        Ok(request_reveal::RequestReveal.process(
            user.info, house.info, spin.info, identity.info, oracle_queue.info, slot_hashes.info,
            system_program.info, vrf_program.info,
        )?)
    }

    /// The VRF oracle's callback.
    #[instruction(discriminator = 19)]
    pub fn callback_reveal<'a>(
        &self,
        vrf_identity: &Signer<'a>,
        spin: &mut Account<'a>,
        args: callback_reveal::CallbackReveal,
    ) -> Result<()> {
        Ok(args.process(vrf_identity.info, spin.info)?)
    }

    #[instruction(discriminator = 20)]
    pub fn hold<'a>(&self, signer: &Signer<'a>, spin: &mut Account<'a>, args: hold::Hold) -> Result<()> {
        Ok(args.process(signer.info, spin.info)?)
    }

    #[instruction(discriminator = 21)]
    pub fn gamble<'a>(&self, signer: &Signer<'a>, spin: &mut Account<'a>) -> Result<()> {
        Ok(gamble::Gamble.process(signer.info, spin.info)?)
    }

    #[instruction(discriminator = 22)]
    pub fn request_collect<'a>(
        &self,
        user: &Account<'a>,
        house: &mut Account<'a>,
        spin: &mut Account<'a>,
        receipt: &mut Account<'a>,
        ephemeral_vault: &mut Account<'a>,
        magic_program: &Account<'a>,
        vault_program: &Account<'a>,
        wallet: &Signer<'a>,
        house_ledger: &mut Account<'a>,
        magic_context: &mut Account<'a>,
    ) -> Result<()> {
        Ok(request_collect::RequestCollect.process(
            user.info, house.info, spin.info, receipt.info, ephemeral_vault.info,
            magic_program.info, vault_program.info, wallet.info, house_ledger.info,
            magic_context.info,
        )?)
    }

    /// The vault's settle callback for a payout.
    #[instruction(discriminator = 23)]
    pub fn resolve_collect<'a>(
        &self,
        receipt: &Account<'a>,
        vault_authority: &Account<'a>,
        house: &mut Account<'a>,
        spin: &mut Account<'a>,
        ephemeral_vault: &mut Account<'a>,
        magic_program: &Account<'a>,
        analytics: &mut Account<'a>,
        args: resolve_collect::ResolveCollect,
    ) -> Result<()> {
        Ok(args.process(
            receipt.info, vault_authority.info, house.info, spin.info, ephemeral_vault.info,
            magic_program.info, analytics.info,
        )?)
    }
}
