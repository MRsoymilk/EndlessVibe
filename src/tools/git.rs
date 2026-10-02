//! Dedicated Git tools are split by responsibility while preserving the public
//! git::status/diff/log/commit/push surface used by MCP handlers.
mod commit;
mod push;
mod recovery;
mod repo;
mod review;

pub use commit::commit;
pub use push::push;
pub use repo::{log,status};
pub use review::diff;
