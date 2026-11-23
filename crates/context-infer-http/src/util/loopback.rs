use std::net::IpAddr;

/// Whether `base_url` names this machine: `localhost` or a loopback address.
pub(crate) fn is_loopback(base_url: &str) -> bool {
    let authority = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_machine_counts_as_loopback() {
        for local in [
            "http://127.0.0.1:8080",
            "http://localhost:8000/",
            "https://[::1]:8443/v1",
            "http://127.1.2.3",
        ] {
            assert!(is_loopback(local), "{local}");
        }
        for remote in [
            "https://api.example.com",
            "https://spark.lan:8443",
            "http://192.168.1.4:8080",
            "http://localhost.example.com",
            "http://127.0.0.1.example.com",
        ] {
            assert!(!is_loopback(remote), "{remote}");
        }
    }
}
