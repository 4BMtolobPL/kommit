use crate::provider::LlmProvider;
use anyhow::{Context, bail};
use url::Url;

/// Resolves the URL and port of the LLM server.
///
/// The host from the command line replaces the one from the config file. The port is
/// taken from the command line first, then from the config file, then from the provider
/// default. Within each of the two layers, the port may come from the explicit port
/// value or from the host, and the two must agree.
pub(crate) fn resolve_endpoint(
    provider: &LlmProvider,
    cli_host: Option<&str>,
    cli_port: Option<u16>,
    config_host: Option<&str>,
    config_port: Option<u16>,
) -> anyhow::Result<(Url, u16)> {
    let cli_url = cli_host.map(parse_host).transpose()?;
    // The config host only matters when the command line does not replace it.
    let config_url = match cli_host {
        Some(_) => None,
        None => config_host.map(parse_host).transpose()?,
    };

    let cli_resolved = pick(
        cli_port,
        cli_host.zip(cli_url.as_ref()),
        |raw, from_host, explicit| {
            format!("--host {raw} specifies port {from_host}, but --port specifies {explicit}")
        },
    )?;
    let config_resolved = pick(
        config_port,
        config_host.zip(config_url.as_ref()),
        |raw, from_host, explicit| {
            format!(
                "config host \"{raw}\" specifies port {from_host}, but config port is {explicit}"
            )
        },
    )?;

    let url = cli_url
        .or(config_url)
        .unwrap_or_else(|| provider.default_host());
    let port = cli_resolved
        .or(config_resolved)
        .unwrap_or_else(|| provider.default_port());
    Ok((url, port))
}

/// Picks the port of one layer, failing if its port value and its host disagree.
fn pick(
    explicit: Option<u16>,
    host: Option<(&str, &Url)>,
    describe: impl Fn(&str, u16, u16) -> String,
) -> anyhow::Result<Option<u16>> {
    let from_host = host.and_then(|(raw, url)| explicit_port(raw, url).map(|port| (raw, port)));
    match (explicit, from_host) {
        (Some(explicit), Some((raw, from_host))) if explicit != from_host => {
            bail!("Conflicting ports: {}", describe(raw, from_host, explicit))
        }
        (Some(port), _) | (None, Some((_, port))) => Ok(Some(port)),
        (None, None) => Ok(None),
    }
}

fn parse_host(raw: &str) -> anyhow::Result<Url> {
    let result = if raw.contains("://") {
        Url::parse(raw)
    } else {
        Url::parse(&format!("http://{raw}"))
    };
    result.with_context(|| format!("Invalid host {raw}"))
}

/// Returns the port written in `raw`, including the default port of the scheme,
/// which `Url::port` leaves out.
fn explicit_port(raw: &str, url: &Url) -> Option<u16> {
    url.port().or_else(|| {
        if has_port(raw) {
            url.port_or_known_default()
        } else {
            None
        }
    })
}

/// Whether the authority of `raw` ends with `:<digits>`.
fn has_port(raw: &str) -> bool {
    let rest = raw.split_once("://").map_or(raw, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let after_host = match host_port.rfind(']') {
        Some(end) => &host_port[end + 1..],
        None => host_port,
    };
    after_host
        .rsplit_once(':')
        .is_some_and(|(_, port)| !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(
        cli_host: Option<&str>,
        cli_port: Option<u16>,
        config_host: Option<&str>,
        config_port: Option<u16>,
    ) -> anyhow::Result<(Url, u16)> {
        resolve_endpoint(
            &LlmProvider::Ollama,
            cli_host,
            cli_port,
            config_host,
            config_port,
        )
    }

    fn port(
        cli_host: Option<&str>,
        cli_port: Option<u16>,
        config_host: Option<&str>,
        config_port: Option<u16>,
    ) -> u16 {
        resolve(cli_host, cli_port, config_host, config_port)
            .unwrap()
            .1
    }

    fn error(
        cli_host: Option<&str>,
        cli_port: Option<u16>,
        config_host: Option<&str>,
        config_port: Option<u16>,
    ) -> String {
        format!(
            "{:#}",
            resolve(cli_host, cli_port, config_host, config_port).unwrap_err()
        )
    }

    // Reproduces issue #45.
    #[test]
    fn port_in_cli_host_is_used() {
        assert_eq!(port(Some("127.0.0.1:18434"), None, None, None), 18434);
    }

    #[test]
    fn url_host_keeps_its_path_and_port() {
        let (url, port) = resolve(Some("http://h:8080/ollama"), None, None, None).unwrap();
        assert_eq!(url.host_str(), Some("h"));
        assert_eq!(url.path(), "/ollama");
        assert_eq!(port, 8080);
    }

    #[test]
    fn host_without_port_falls_back_in_order() {
        assert_eq!(port(Some("h"), Some(1), Some("x"), Some(2)), 1);
        assert_eq!(port(Some("h"), None, Some("x"), Some(2)), 2);
        assert_eq!(port(Some("h"), None, None, None), 11434);
        assert_eq!(port(None, None, None, None), 11434);
        let (url, port) = resolve_endpoint(&LlmProvider::LmStudio, None, None, None, None).unwrap();
        assert_eq!(url, LlmProvider::LmStudio.default_host());
        assert_eq!(port, 1234);
    }

    #[test]
    fn cli_layer_beats_config_layer() {
        assert_eq!(port(Some("h:1"), None, None, Some(2)), 1);
        assert_eq!(port(Some("h:1"), None, Some("x:2"), Some(2)), 1);
    }

    #[test]
    fn replaced_config_host_does_not_supply_a_port() {
        assert_eq!(port(Some("h"), None, Some("x:9"), None), 11434);
        // Nor can it conflict with the config port.
        assert_eq!(port(Some("h"), None, Some("x:9"), Some(2)), 2);
    }

    #[test]
    fn port_in_config_host_is_used() {
        let (url, port) = resolve(None, None, Some("x:18434"), None).unwrap();
        assert_eq!(url.host_str(), Some("x"));
        assert_eq!(port, 18434);
    }

    #[test]
    fn conflicting_cli_ports_are_an_error() {
        let message = error(Some("127.0.0.1:18434"), Some(11434), None, None);
        assert!(message.contains("Conflicting ports"), "{message}");
        assert!(message.contains("18434"), "{message}");
        assert!(message.contains("11434"), "{message}");
        assert!(message.contains("--port"), "{message}");
    }

    #[test]
    fn conflicting_config_ports_are_an_error() {
        let message = error(None, None, Some("x:18434"), Some(11434));
        assert!(message.contains("Conflicting ports"), "{message}");
        assert!(message.contains("18434"), "{message}");
        assert!(message.contains("11434"), "{message}");
        assert!(message.contains("config port"), "{message}");
    }

    #[test]
    fn matching_ports_are_allowed() {
        assert_eq!(port(Some("h:5"), Some(5), None, None), 5);
        assert_eq!(port(None, None, Some("x:5"), Some(5)), 5);
    }

    #[test]
    fn explicit_default_scheme_port_is_kept() {
        assert_eq!(port(Some("http://h:80"), None, None, None), 80);
        assert_eq!(port(Some("https://h:443"), None, None, None), 443);
        assert_eq!(port(Some("h:80"), None, None, None), 80);
        let message = error(Some("http://h:80"), Some(11434), None, None);
        assert!(message.contains("Conflicting ports"), "{message}");
    }

    #[test]
    fn ipv6_and_userinfo_are_handled() {
        assert_eq!(port(Some("[::1]:18434"), None, None, None), 18434);
        assert_eq!(port(Some("[::1]"), None, None, None), 11434);
        assert_eq!(port(Some("http://user:pw@h"), None, None, None), 11434);
        assert_eq!(port(Some("http://user:pw@h:80/a:1"), None, None, None), 80);
    }

    #[test]
    fn invalid_host_is_an_error() {
        let message = error(Some("h:99999"), None, None, None);
        assert!(message.contains("Invalid host h:99999"), "{message}");
    }
}
