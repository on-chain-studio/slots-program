use borsh::BorshDeserialize;
use solana_program::{account_info::AccountInfo, program_error::ProgramError, pubkey::Pubkey, entrypoint::ProgramResult};

use crate::error::GameError;
use crate::instruction::{ix, ProcessInstruction};
use crate::state::config::{MODE_GAMBLE, MODE_HOLD};
use crate::state::spin::{Spin, SpinStatus};
use crate::utils::{engine, pda, receipt, vault};

/// Prices the finished spin and asks the vault to pay it. The session key fires this
/// automatically — on a hold machine it also works mid-sequence, since collecting grid N equals
/// holding every reel and respinning, so stopping early forfeits nothing.
///
/// Nothing about the spin is written here. A spin is collected when it is *gone* — the settle
/// callback closes it — so asking for a payout leaves no state to strand. Load-bearing, exactly
/// as it was for cards: a version that marked the spin spent would let anyone flip a stranger's
/// finished spin and never settle, destroying the win and blocking that player for good.
/// Accounts: [user, house, spin, receipt, ephemeral_vault, magic_program, vault_program,
///            wallet (signer), house_ledger, magic_context]
#[derive(BorshDeserialize)]
pub struct RequestCollect;

/// What a finished spin pays, given everything the account holds. Pure, and shared with
/// `resolve_collect`, so the number the receipt names, the number the vault moves and the number
/// analytics records are all the same computation.
pub fn payout(spin: &Spin, terms: &crate::state::MachineConfig) -> Result<u64, ProgramError> {
    match terms.mode {
        MODE_GAMBLE if spin.round > 0 => {
            // On the ladder: the pending seed is a flip over what rides.
            Ok(if engine::gamble(terms, &spin.seed)? {
                spin.pending.checked_mul(2).ok_or(ProgramError::ArithmeticOverflow)?
            } else {
                0
            })
        }
        MODE_HOLD => {
            let stops = engine::spin(terms, &spin.seed, spin.hold as u8, &spin.stops())?;
            Ok(engine::value(terms, &stops)?.lamports)
        }
        _ => {
            let stops = engine::spin(terms, &spin.seed, 0, &spin.stops())?;
            Ok(engine::value(terms, &stops)?.lamports)
        }
    }
}

impl ProcessInstruction for RequestCollect {
    fn process(&self, program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
        let [user, house, spin_account, receipt_account, ephemeral_vault, magic_program,
             vault_program, wallet, house_ledger, magic_context, ..] = accounts else {
            return Err(ProgramError::NotEnoughAccountKeys);
        };

        // A win may open a new token slot on the player's ledger; the vault needs the owner's
        // consent, which is the session key signing here.
        if !wallet.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let house_bump = pda::validate(program_id, house, &[b"house"])?;
        if *house_ledger.key != vault::ledger(house.key) {
            return Err(GameError::InvalidPDA.into());
        }
        if *receipt_account.key != receipt::address(wallet.key) {
            return Err(GameError::InvalidPDA.into());
        }

        let terms = *Spin::terms(spin_account)?;
        let amount = {
            let spin = Spin::load(spin_account)?;
            if spin.user != user.key.to_bytes() {
                return Err(GameError::Unauthorized.into());
            }
            if spin.status != SpinStatus::Rolled as u64 {
                return Err(GameError::NotRolled.into());
            }
            pda::validate(program_id, spin_account, &[b"spin", user.key.as_ref()])?;
            let amount = payout(spin, &terms)?;
            // While a round decision is still open — a hold respin, or a gamble win that can
            // ride further — only the consenter may collect. Otherwise a stranger could
            // force-settle the current grid and rob the player of their remaining respins or
            // ladder climbs. Once nothing is left to decide, collect stays permissionless so an
            // abandoned finished spin can still be cranked closed by anyone.
            let deciding = match terms.mode {
                MODE_HOLD => spin.round + 1 < terms.rounds() as u64,
                MODE_GAMBLE => amount > 0 && spin.round < terms.gamble_rungs as u64,
                _ => false,
            };
            if deciding && wallet.key.to_bytes() != spin.consenter {
                return Err(GameError::Unauthorized.into());
            }
            amount
        };

        // A loss is an empty receipt: the settle moves nothing and still fires the callback,
        // which is what closes the spin.
        let mut movements = Vec::new();
        if amount > 0 {
            movements.push(receipt::Movement {
                mint: Pubkey::new_from_array(terms.mint),
                amount,
                from: 1,
                to: 0,
            });
        }

        receipt::create(
            vault_program, house, house_ledger, wallet, receipt_account, ephemeral_vault,
            magic_program, magic_context,
            program_id,
            &[b"house", &[house_bump]],
            &[*user.key, *house.key],
            ix::ResolveCollect,
            &[],
            &movements,
        )
    }
}
