use std::sync::OnceLock;

/// A seed for the tile and line hashes, new in each process, so a hash in memory cannot be
/// matched against a table of known screen content.
pub(crate) fn hash_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        let mut bytes = [0_u8; 8];
        if getrandom::fill(&mut bytes).is_err() {
            tracing::warn!("no random hash seed; content hashes use a fixed one");
        }
        u64::from_le_bytes(bytes)
    })
}
