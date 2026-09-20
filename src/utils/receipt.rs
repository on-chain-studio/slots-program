
use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult, instruction::{AccountMeta, Instruction},
    program::invoke_signed, program_error::ProgramError, pubkey::Pubkey,
};

use crate::constants::VAULT_PROGRAM;
use crate::error::GameError;

/// Anchor's `sha256("global:create_receipt")[..8]`.
const CREATE_RECEIPT_DISC: [u8; 8] = [187, 57, 104, 13, 15, 1, 219, 99];

pub const MOVEMENT_SIZE: usize = 32 + 8 + 1 + 1;

pub struct Movement {
    pub mint: Pubkey,
    pub amount: u64,
    /// Indices into the receipt's ledgers: 0 is the player, 1.. are our own PDAs in the order
    /// their seeds are passed to `create`.
    pub from: u8,
    pub to: u8,
}

fn seeds_arg(seeds: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(seeds.len() as u32).to_le_bytes());
    for s in seeds {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s);
    }
    out
}

/// Touches no ledger, so it can run as a game CPI in the rollup (a ledger there fails the ACL);
/// consent is checked at settle. Seeded and signed by the `consenter`, so it can't be minted for another.
#[allow(clippy::too_many_arguments)]
pub fn create<'a>(
    vault_program: &AccountInfo<'a>,
    authority: &AccountInfo<'a>,
    authority_ledger: &AccountInfo<'a>,
    consenter: &AccountInfo<'a>,
    receipt: &AccountInfo<'a>,
    ephemeral_vault: &AccountInfo<'a>,
    magic_program: &AccountInfo<'a>,
    magic_context: &AccountInfo<'a>,
    member_program: &Pubkey,
    authority_seeds: &[&[u8]],
    owners: &[Pubkey],
    callback_disc: u64,
    args: &[u8],
    movements: &[Movement],
) -> ProgramResult {
    let mut data =
        Vec::with_capacity(64 + movements.len() * MOVEMENT_SIZE + owners.len() * 32 + args.len());
    data.extend_from_slice(&CREATE_RECEIPT_DISC);
    data.extend_from_slice(&(movements.len() as u32).to_le_bytes());
    for m in movements {
        data.extend_from_slice(m.mint.as_ref());
        data.extend_from_slice(&m.amount.to_le_bytes());
        data.push(m.from);
        data.push(m.to);
    }
    data.extend_from_slice(member_program.as_ref());
    data.extend_from_slice(&seeds_arg(authority_seeds));
    data.extend_from_slice(&(owners.len() as u32).to_le_bytes());
    for o in owners {
        data.extend_from_slice(o.as_ref());
    }
    data.extend_from_slice(&callback_disc.to_le_bytes());
    data.extend_from_slice(&(args.len() as u32).to_le_bytes());
    data.extend_from_slice(args);

    invoke_signed(
        &Instruction {
            program_id: VAULT_PROGRAM,
            accounts: vec![
                AccountMeta::new(*authority.key, true),
                AccountMeta::new(*authority_ledger.key, false),
                AccountMeta::new_readonly(*consenter.key, true),
                AccountMeta::new(*receipt.key, false),
                AccountMeta::new(*ephemeral_vault.key, false),
                AccountMeta::new_readonly(*magic_program.key, false),
                AccountMeta::new(*magic_context.key, false),
            ],
            data,
        },
        &[
            vault_program.clone(), authority.clone(), authority_ledger.clone(),
            consenter.clone(), receipt.clone(), ephemeral_vault.clone(),
            magic_program.clone(), magic_context.clone(),
        ],
        &[authority_seeds],
    )
}

/// The vault's seedless authority — its signature is what marks a settle callback as genuine.
pub const VAULT_AUTHORITY: Pubkey =
    solana_program::pubkey!("341xevm3ejTyZCncco8UdEuiagcBbZQtJnEgsDDYBcgs");

pub fn require_callback(vault_authority: &AccountInfo) -> Result<(), ProgramError> {
    if !vault_authority.is_signer || vault_authority.key != &VAULT_AUTHORITY {
        return Err(GameError::NotPaid.into());
    }
    Ok(())
}

pub fn close<'a>(
    magic_program: &AccountInfo<'a>,
    house: &AccountInfo<'a>,
    receipt: &AccountInfo<'a>,
    ephemeral_vault: &AccountInfo<'a>,
    house_bump: u8,
) -> ProgramResult {
    invoke_signed(
        &Instruction {
            program_id: *magic_program.key,
            accounts: vec![
                AccountMeta::new(*house.key, true),
                AccountMeta::new(*receipt.key, false),
                AccountMeta::new(*ephemeral_vault.key, false),
            ],
            // MagicBlockInstruction::CloseEphemeralAccount = variant 14 (bincode u32 LE)
            data: 14u32.to_le_bytes().to_vec(),
        },
        &[house.clone(), receipt.clone(), ephemeral_vault.clone()],
        &[&[b"house", &[house_bump]]],
    )
}

/// `["receipt", program, consenter]` — the session key's receipt for this program.
pub fn address(consenter: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[b"receipt", crate::entrypoint::ID.as_ref(), consenter.as_ref()],
        &VAULT_PROGRAM,
    )
    .0
}
