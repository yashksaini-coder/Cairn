pub mod close_escrow;
pub mod create_escrow;
pub mod refund;
pub mod submit_receipt;

// Glob, not named re-exports. `#[derive(Accounts)]` also generates
// `__client_accounts_*` helper modules beside each struct, and `#[program]`
// resolves them at the crate root -- so naming only the structs leaves the
// macro with an unresolved `crate::` import that reads like a bug in Anchor.
//
// Hence also the `handle_` prefix on the handlers. They cannot all be called
// `handler` (four of those in one namespace is ambiguous), and they cannot be
// named after their instruction either -- `#[program]` exports functions by
// those exact names at the crate root, and the glob would collide with them.
pub use close_escrow::*;
pub use create_escrow::*;
pub use refund::*;
pub use submit_receipt::*;
