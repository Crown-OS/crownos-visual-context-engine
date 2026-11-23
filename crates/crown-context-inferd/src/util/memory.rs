use context_infer::registry::Tier;

/// The inference tier from the installed memory: 30 GiB and up is high, 14 GiB mid.
pub(crate) fn memory_tier() -> Tier {
    let kibibytes = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|meminfo| {
            meminfo
                .lines()
                .find_map(|line| line.strip_prefix("MemTotal:"))
                .and_then(|value| {
                    value
                        .trim()
                        .trim_end_matches("kB")
                        .trim()
                        .parse::<u64>()
                        .ok()
                })
        })
        .unwrap_or(0);
    match kibibytes >> 20 {
        30.. => Tier::High,
        14.. => Tier::Mid,
        _ => Tier::Low,
    }
}
