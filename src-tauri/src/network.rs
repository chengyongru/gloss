use crate::app_state::AppState;
use reqwest::{Client, Proxy};
use std::time::Duration;
use url::Url;

pub fn build_client(
    state: &AppState,
    connect_timeout: Duration,
    timeout: Duration,
) -> Result<Client, String> {
    let proxy_url = state
        .settings
        .lock()
        .map_err(|_| "The settings state is unavailable.".to_owned())?
        .proxy_url
        .clone();
    let mut builder = Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(timeout);
    if !proxy_url.is_empty() {
        let proxy = Proxy::all(&proxy_url)
            .map_err(|_| "The saved proxy URL is invalid. Check Network settings.".to_owned())?;
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(|_| {
        "Gloss couldn't create a network connection. Check the proxy setting.".to_owned()
    })
}

pub fn normalize_proxy_url(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    let candidate = if value.contains("://") {
        value.to_owned()
    } else {
        format!("http://{value}")
    };
    let parsed = Url::parse(&candidate)
        .map_err(|_| "Proxy URL is invalid. Try 127.0.0.1:23458.".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h") {
        return Err("Proxy must use HTTP, HTTPS, SOCKS5, or SOCKS5H.".to_owned());
    }
    if parsed.host_str().is_none() {
        return Err("Proxy URL needs a host name or IP address.".to_owned());
    }
    if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() || parsed.fragment().is_some()
    {
        return Err("Proxy URL cannot contain a path, query, or fragment.".to_owned());
    }
    Proxy::all(parsed.as_str())
        .map_err(|_| "Proxy URL is not supported by the network client.".to_owned())?;

    let mut normalized = parsed.to_string();
    if normalized.ends_with('/') {
        normalized.pop();
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::normalize_proxy_url;

    #[test]
    fn normalizes_local_http_proxy_shorthand() {
        assert_eq!(
            normalize_proxy_url(" 127.0.0.1:23458 ").unwrap(),
            "http://127.0.0.1:23458"
        );
    }

    #[test]
    fn accepts_supported_proxy_schemes() {
        assert_eq!(
            normalize_proxy_url("socks5h://localhost:1080").unwrap(),
            "socks5h://localhost:1080"
        );
        assert_eq!(normalize_proxy_url("  ").unwrap(), "");
    }

    #[test]
    fn rejects_non_proxy_urls() {
        assert!(normalize_proxy_url("ftp://localhost:21").is_err());
        assert!(normalize_proxy_url("http://localhost:8080/path").is_err());
    }
}
