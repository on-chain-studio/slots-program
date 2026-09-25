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

/// Just the house. There is no progressive jackpot here: with no pot to feed, the whole return
/// target lives in the machines themselves — 90% RTP, where scratch cards run 80% + 10% pot.
pub const TREASURIES: [&[u8]; 1] = [b"house"];

impl Casino for Slots {
    const ID: Pubkey = crate::ID;
    /// The ops key alone: it is also the upgrade authority and so is kept private anyway. The dev
    /// key is handled like a hot wallet — it may read the books (`ANALYTICS_READERS`) but it must
    /// never move or reprice anything.
    const ADMINS: &'static [Pubkey] = &OPS_ONLY;
    const TREASURIES: &'static [&'static [u8]] = &TREASURIES;
}
