use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Builds a scratch directory path under the OS temp dir that nothing else
/// can claim: the caller-supplied `label` is combined with this process's id
/// and an atomically incremented counter, so the name is unique both across
/// concurrent `cargo test` processes (e.g. two collaborators or CI jobs
/// sharing a runner) and across parallel test threads within one process.
///
/// The directory is not created; callers still own creating and removing it.
pub fn unique_temp_dir(label: &str) -> PathBuf {
    let pid = std::process::id();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("wso_{label}_{pid}_{n}"))
}
