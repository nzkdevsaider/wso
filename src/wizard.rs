//! Interactive prompts. Fully implemented in Task 14.
//!
//! This stub exists only so the CLI wired up in Task 13 has something to
//! call: it returns immediately, changes nothing in `cfg`, and never
//! cancels. `sync` skips this entirely whenever the invocation already
//! carried enough flags (see `Cli::is_non_interactive`), so on this machine,
//! today, it is effectively dead code — but the tree must still compile and
//! link against a real `wizard::run`.

use crate::config::Config;

/// Returned when the user backs out of the wizard without choosing
/// anything. The stub never produces this.
pub struct Cancelled;

pub fn run(_cfg: &mut Config) -> Result<(), Cancelled> {
    Ok(())
}
