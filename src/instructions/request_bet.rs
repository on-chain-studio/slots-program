use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::error::GameError;
use crate::instruction::{ix, ProcessInstruction};
use crate::state::Config;
use crate::utils::{pda, receipt, vault};

/// Places a bet: a receipt for the whole stake, player → house. The settle callback creates the
/// ephemeral `["spin", user]` with its terms copied from the shelf — so the bet exists if and
/// only if it is paid for, and a dropped settle costs nothing and creates nothing.
/// Accounts: [wallet (signer, consents), user, config, house, receipt, ephemeral_vault,
///            magic_program, vault_program, house_ledger, magic_context]
#[derive(BorshDeserialize)]
pub struct RequestBet {
    pub machine_id: u64,
}

impl ProcessInstruction for RequestBet {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [wallet, user, config_account, house, receipt_account, ephemeral_vault,
             magic_program, vault_program, house_ledger, magic_context, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        if !wallet.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }
        pda::validate(program_id, config_account, &[b"config"])?;
        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        if *house_ledger.key != vault::ledger(house.key) {
            return Err(GameError::InvalidPDA.into());
        }
        if *receipt_account.key != receipt::address(wallet.key) {
            return Err(GameError::InvalidPDA.into());
        }

        let machine = Config::machine(config_account, self.machine_id)?;
        let stake = machine.stake_lamports;
        let mint = Pubkey::new_from_array(machine.mint);

        // The whole stake to the house — no jackpot cut here; the return target lives in the
        // machine itself.
        let movements = vec![receipt::Movement { mint, amount: stake, from: 0, to: 1 }];

        // The consenter rides the args so the callback can record it on the spin: the vault is
        // about to prove this key may act for the user, and the round decisions reuse that proof.
        let mut args = Vec::with_capacity(8 + 32);
        args.extend_from_slice(&self.machine_id.to_le_bytes());
        args.extend_from_slice(wallet.key.as_ref());

        receipt::create(
            vault_program, house, house_ledger, wallet, receipt_account, ephemeral_vault,
            magic_program, magic_context,
            program_id,
            &[b"house", &[house_bump]],
            &[*user.key, *house.key],
            ix::ResolveBet,
            &args,
            &movements,
        )
    }
}
