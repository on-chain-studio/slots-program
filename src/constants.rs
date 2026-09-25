use crate::chain::*;

/// The only key allowed to sign admin instructions: the mainnet ops key, which is also the
/// upgrade authority and so is kept private anyway. The dev key is handled like a hot wallet —
/// it may read the books (see `ANALYTICS_READERS`) but it must never move or reprice anything.
pub const ADMIN_PUBKEYS: [Pubkey; 1] = [
    Pubkey::from_str_const("2wpqngzMS3CUu6LMaL6M3ykgBGPoXRwP4Ps8TLhx5FZH"),
];

/// Who may read the analytics counters on the TEE: the dev key (its token feeds the hosted
/// analytics watcher) and the ops key.
pub const ANALYTICS_READERS: [Pubkey; 2] = [
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

/// MagicBlock VRF program. Callbacks are signed by the scoped identity `["identity", program]`
/// at the VRF program — `utils::vrf::callback_identity`.
pub const VRF_PROGRAM: Pubkey = Pubkey::from_str_const("Vrf1RNUjXmQGjmQrQLvJHs9SNkvDJEsRVFPkfSQUwGz");
