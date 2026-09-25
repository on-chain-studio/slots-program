use borsh::{BorshDeserialize, BorshSerialize};
use casino_core::chain::*;
use casino_core::{pda, receipt, vault, CoreError};

use crate::state::Config;

/// Places a bet: a receipt for the whole stake, player → house. The settle callback creates the
/// ephemeral `["spin", user]` with its terms copied from the shelf — so the bet exists if and
/// only if it is paid for, and a dropped settle costs nothing and creates nothing.
/// Accounts: [wallet (signer, consents), user, config, house, receipt, ephemeral_vault,
///            magic_program, vault_program, house_ledger, magic_context]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct RequestBet {
    pub machine_id: u64,
}

impl RequestBet {
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub fn process<'a>(
        &self,
        wallet: &AccountInfo,
        user: &AccountInfo,
        config_account: &AccountInfo,
        house: &AccountInfo,
        receipt_account: &AccountInfo,
        ephemeral_vault: &AccountInfo,
        magic_program: &AccountInfo,
        vault_program: &AccountInfo,
        house_ledger: &AccountInfo,
        magic_context: &AccountInfo,
    ) -> ProgramResult {
        let program_id = &crate::ID;

        if !wallet.is_signer() {
            return Err(ProgramError::MissingRequiredSignature);
        }
        pda::validate(program_id, config_account, &[b"config"])?;
        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        if *house_ledger.address() != vault::ledger(house.address()) {
            return Err(CoreError::InvalidPDA.into());
        }
        if *receipt_account.address() != receipt::address(program_id, wallet.address()) {
            return Err(CoreError::InvalidPDA.into());
        }

        let machine = Config::item(config_account, self.machine_id)?;
        let stake = machine.stake_lamports;
        let mint = Pubkey::new_from_array(machine.mint);

        // The whole stake to the house — no jackpot cut here; the return target lives in the
        // machine itself.
        let movements = vec![receipt::Movement { mint, amount: stake, from: 0, to: 1 }];

        // The consenter rides the args so the callback can record it on the spin: the vault is
        // about to prove this key may act for the user, and the round decisions reuse that proof.
        let mut args = Vec::with_capacity(8 + 32);
        args.extend_from_slice(&self.machine_id.to_le_bytes());
        args.extend_from_slice(wallet.address().as_ref());

        receipt::create(
            vault_program, house, house_ledger, wallet, receipt_account, ephemeral_vault,
            magic_program, magic_context,
            program_id,
            &[b"house", &[house_bump]],
            &[*user.address(), *house.address()],
            crate::SlotsInstruction::RESOLVE_BET,
            &args,
            &movements,
        )
    }
}
