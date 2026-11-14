use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, fmt};

const JOURNAL_IDENTIFIER: &str = "crown-contextd";

/// The terminal and, when journald listens, the journal.
pub(crate) fn init() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let journal = tracing_journald::layer()
        .ok()
        .map(|layer| layer.with_syslog_identifier(JOURNAL_IDENTIFIER.to_owned()));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .with(journal)
        .init();
}
