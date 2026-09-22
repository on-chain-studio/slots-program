use crate::chain::*;

/// Every key allowed to sign admin instructions: the dev key, and the mainnet ops key.
pub const ADMIN_PUBKEYS: [Pubkey; 2] = [
    Pubkey::from_str_const("691aFvKMnHXrMSgqk6G8izoCbVZTmkrRcu8xCeMKfPh1"),
    Pubkey::from_str_const("2wpqngzMS3CUu6LMaL6M3ykgBGPoXRwP4Ps8TLhx5FZH"),
];

pub fn is_admin(key: &Pubkey) -> bool {
    ADMIN_PUBKEYS.contains(key)
}

/// Just the house. There is no progressive jackpot here: with no pot to feed, the whole return
/// target lives in the machines themselves — 90% RTP, where scratch cards run 80% + 10% pot.
pub const TREASURIES: [&[u8]; 1] = [b"house"];

pub fn treasury_seed(which: u8) -> Result<&'static [u8], ProgramError> {
    TREASURIES
        .get(which as usize)
        .copied()
        .ok_or(ProgramError::InvalidInstructionData)
}

/// The vault program — holds every balance this game touches. The game owns no token accounts:
/// a ledger per treasury, and payment is a `settle`. Shared with scratch cards, which is why a
/// player who funded there can spin here without funding again: a ledger is `["ledger", owner]`,
/// keyed on the wallet, not on the game.
pub const VAULT_PROGRAM: Pubkey = Pubkey::from_str_const("VAULTrDSUBZ8AXL2kGVYE8eKAn7tgWXRPAevNGUsyTV");

pub const TOKEN_PROGRAM: Pubkey = Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

/// MagicBlock's access-control program — permissions gating who may read an account on the TEE.
pub const PERMISSION_PROGRAM: Pubkey = Pubkey::from_str_const("ACLseoPoyC3cBqoUtkbjZ4aDrkurZW86v19pXz2XQnp1");

/// MagicBlock VRF program and its identity signer for callbacks.
pub const VRF_PROGRAM:          Pubkey = Pubkey::from_str_const("Vrf1RNUjXmQGjmQrQLvJHs9SNkvDJEsRVFPkfSQUwGz");
pub const VRF_PROGRAM_IDENTITY: Pubkey = Pubkey::from_str_const("9irBy75QS2BN81FUgXuHcjqceJJRuc9oDkAe8TKVvvAw");
