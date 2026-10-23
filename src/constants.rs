//! What this program says about itself. Everything else it calls — the vault, MagicBlock, the
//! VRF — is a property of the chain and lives in `casino_core::ids`, and so do the keys that
//! operate every game on the shelf.

use casino_core::chain::*;
use casino_core::ids::{DEV_KEY, OPS_KEY, OPS_ONLY};
use casino_core::Casino;

use crate::Slots;

/// Who may read the analytics counters on the TEE: the dev key (its token feeds the hosted
/// analytics watcher) and the ops key. Reading is all the dev key does here — it signs no admin
/// instruction, see `ADMINS` below.
pub const ANALYTICS_READERS: [Pubkey; 2] = [DEV_KEY, OPS_KEY];

/// The key MagicBlock Quests' lookup signs `HasPlayed` with. It holds nothing and signs only
/// simulations; whoever has it learns whether a wallet has ever spun, and nothing else.
pub const QUEST_VERIFIER: Pubkey = Pubkey::from_str_const("4cd9zCgws9FKUmkLudUnyXzszc5zh71i1LFRerWkwcVr");

/// The casino floor program. It is top-level over spins, so it must be a member of their permission.
pub const PRIVATE_CASINO: Pubkey = Pubkey::from_str_const("EzQPZpLj8DccG7M7VCyr9kEm4wFLZi6pDGfxrAsLNz6z");

/// `["analytics"]`, pinned so the payout task's instruction, built in `RequestCollect`, need not
/// derive it.
pub const ANALYTICS: Pubkey = Pubkey::from_str_const("68XX3KGW2DjKgXPt9kUzq8pvUAdMze4BZzs3QhrVL6aa");

/// The house's payout task fires once. A payout that cannot be made is left uncollected, in the
/// open on the play's trailer, rather than retried in the hope it fixes itself.
pub const PAY_MS: u64 = 1;
pub const PAY_ATTEMPTS: u64 = 1;

/// Just the house. There is no progressive jackpot here: with no pot to feed, the whole return
/// target lives in the machines themselves — 90% RTP, where scratch cards run 80% + 10% pot.
pub const TREASURIES: [&[u8]; 1] = [b"house"];

impl Casino for Slots {
    const ID: Pubkey = crate::ID;
    /// The ops key alone: it is also the upgrade authority and so is kept private anyway. The dev
    /// key is handled like a hot wallet — it may read the books (`ANALYTICS_READERS`) but it must
    /// never move or reprice anything.
    const ADMINS: &'static [Pubkey] = &OPS_ONLY;
    const CALLERS: &'static [Pubkey] = &[PRIVATE_CASINO];
    const TREASURIES: &'static [&'static [u8]] = &TREASURIES;
}
