//! Stands in for every program the game calls — vault, VRF, delegation, magic, permission,
//! token — and writes down what it was asked: its own id, the instruction data, and each
//! account's key with its signer and writable flags, as one `Program data:` log line. Two builds
//! of the game that make different calls leave different logs.
//!
//! One call is also carried out: the magic program's `CreateEphemeralAccount`, since what a game
//! does next depends on the account existing. The account must start out owned by the magic
//! program (a test sets that up); it is sized as asked and handed to the sponsor's owner — the
//! game — which is what the real program does inside a rollup.

use solana_program::{
    account_info::AccountInfo, entrypoint, entrypoint::ProgramResult, log::sol_log_data,
    pubkey, pubkey::Pubkey,
};

const MAGIC: Pubkey = pubkey!("Magic11111111111111111111111111111111111111");

/// `MagicBlockInstruction::CreateEphemeralAccount { data_len: u32 }`, bincode.
const CREATE_EPHEMERAL_ACCOUNT: u32 = 12;

entrypoint!(record);

fn record(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    let mut metas = Vec::with_capacity(accounts.len() * 34);
    for account in accounts {
        metas.extend_from_slice(account.key.as_ref());
        metas.push(account.is_signer as u8);
        metas.push(account.is_writable as u8);
    }
    sol_log_data(&[program_id.as_ref(), data, &metas]);

    if *program_id == MAGIC && data.len() == 8 && accounts.len() >= 2 {
        let variant = u32::from_le_bytes(data[..4].try_into().unwrap());
        let data_len = u32::from_le_bytes(data[4..].try_into().unwrap());
        let (sponsor, ephemeral) = (&accounts[0], &accounts[1]);
        if variant == CREATE_EPHEMERAL_ACCOUNT && ephemeral.owner == program_id && ephemeral.data_is_empty() {
            ephemeral.resize(data_len as usize)?;
            ephemeral.assign(sponsor.owner);
        }
    }
    Ok(())
}
