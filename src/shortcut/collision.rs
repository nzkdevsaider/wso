//! The one place that decides whether it is safe to write over whatever
//! already sits at a shortcut's destination path.
//!
//! This started life inside `migrate`, where converting `Terraria.url` into
//! `Terraria.lnk` could land on a file some earlier run — or the user — had
//! already put there. But `sync` writes to destination paths too, and for a
//! long time it did so unconditionally: `ops::apply`'s `Create` and `Move`
//! branches called `write_lnk` with no idea what they were replacing. A
//! hand-made `Terraria.lnk` was silently overwritten, target and description
//! and all, and the run reported it as a successful create with zero
//! failures. There is no Recycle Bin on that path — the file is gone.
//!
//! That falsified the tool's central promise, stated in `README.md`: `sync`
//! never modifies or removes a file it did not create. So the check lives
//! here now, shared by both commands, rather than in the one module that
//! happened to learn it first.
//!
//! Ownership is decided by exactly the same marker everything else in this
//! crate keys on (`shortcut::signature`), so there is one definition of
//! "ours" in the codebase, not two that could drift apart.

use crate::shortcut::signature::{Signature, decode};
use crate::shortcut::writer::read_lnk;
use std::path::Path;

/// What a shortcut's destination path already holds, and what that means for
/// whether it is safe to overwrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Collision {
    /// Nothing sits at the destination path yet.
    Clear,
    /// The destination already carries our signature for this exact game —
    /// most likely a leftover from a previous `sync` or `migrate`, or the
    /// new file left behind by a `Move` whose cleanup half failed. Safe to
    /// overwrite: this run would write an equivalent shortcut anyway.
    OwnedBySame,
    /// The destination exists and must not be touched: either it carries no
    /// `[wso:...]` ownership marker at all (hand-made, or from some other
    /// tool) or it is signed for a different game entirely. `String`
    /// explains which, for the message shown to the user.
    Blocked(String),
}

/// The pure decision behind [`Collision`], kept independent of any
/// filesystem or COM access so it is unit-testable without a real `.lnk`
/// file: given what the destination's description field already contains
/// (`None` when nothing is there yet), decide whether writing over it is
/// safe.
pub fn classify_existing(existing_description: Option<&str>, expected: &Signature) -> Collision {
    let Some(desc) = existing_description else {
        return Collision::Clear;
    };
    match decode(desc) {
        Some(sig) if sig == *expected => Collision::OwnedBySame,
        Some(sig) => Collision::Blocked(format!(
            "already contains a shortcut for a different game ([wso:{}:{}])",
            sig.platform.as_str(),
            sig.id
        )),
        None => Collision::Blocked("already exists and was not created by this tool".to_string()),
    }
}

/// Reads whatever currently sits at `dest`, if anything, and classifies it.
/// A file that exists but cannot be read as a shell link at all (foreign
/// data, or genuinely corrupt) is treated as blocked rather than clear —
/// the safe direction, since this function must never claim something is
/// safe to overwrite when it could not actually verify that.
pub fn check_existing(dest: &Path, expected: &Signature) -> Collision {
    if !dest.is_file() {
        return Collision::Clear;
    }
    match read_lnk(dest) {
        Ok(data) => classify_existing(Some(&data.description), expected),
        Err(_) => Collision::Blocked("already exists but could not be read".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Platform;

    fn sig(id: &str) -> Signature {
        Signature {
            platform: Platform::Steam,
            id: id.into(),
        }
    }

    /// Nothing at `dest` yet: the ordinary case, always safe to proceed.
    #[test]
    fn classify_existing_is_clear_when_nothing_is_there() {
        assert_eq!(classify_existing(None, &sig("105600")), Collision::Clear);
    }

    /// The safe branch: a previous `sync` (or an earlier `migrate` run)
    /// already left a `.lnk` signed for this *same* game. Overwriting it is
    /// not "silently overwriting" — the file this run would write is
    /// functionally the same shortcut, just refreshed.
    #[test]
    fn classify_existing_allows_overwriting_a_shortcut_owned_for_the_same_game() {
        let existing = crate::shortcut::signature::encode("Terraria", &sig("105600"));
        assert_eq!(
            classify_existing(Some(&existing), &sig("105600")),
            Collision::OwnedBySame
        );
    }

    /// The blocked branch: a `.lnk` signed for a *different* game must never
    /// be silently clobbered.
    #[test]
    fn classify_existing_blocks_a_shortcut_owned_for_a_different_game() {
        let existing = crate::shortcut::signature::encode("Some Other Game", &sig("999"));
        let result = classify_existing(Some(&existing), &sig("105600"));
        assert!(matches!(result, Collision::Blocked(_)), "got {result:?}");
    }

    /// The other blocked branch: a foreign shortcut (hand-made, or from
    /// another tool entirely, carrying no `[wso:...]` marker at all) must
    /// never be treated as safe to overwrite.
    #[test]
    fn classify_existing_blocks_a_shortcut_with_no_ownership_marker() {
        let result = classify_existing(Some("My own shortcut"), &sig("105600"));
        assert!(matches!(result, Collision::Blocked(_)), "got {result:?}");
    }
}
