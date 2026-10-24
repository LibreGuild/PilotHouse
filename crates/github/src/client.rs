//! A small, blocking GitHub REST client (call it from a background thread).
//!
//! TLS goes through the operating system (native-tls with the system's
//! trusted roots), so GitHub Enterprise behind a company CA works when the
//! CA is installed on the machine. The token is sent only to the account's
//! own API host.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use serde::de::DeserializeOwned;
use serde_json::Value;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

/// Most items read from a paged list.
pub const MAX_ITEMS: usize = 1_000;

pub struct Client {
    base: String,
    token: String,
    agent: ureq::Agent,
}

impl Client {
    /// `base` is the API root: `https://api.github.com` or
    /// `https://HOST/api/v3` for GitHub Enterprise Server.
    pub fn new(base: &str, token: &str) -> Self {
        let tls = TlsConfig::builder()
            .provider(TlsProvider::NativeTls)
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .tls_config(tls)
            .timeout_global(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .user_agent(concat!("PilotHouse/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self {
            base: base.trim_end_matches('/').to_string(),
            token: token.to_string(),
            agent,
        }
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with("https://") || path.starts_with("http://") {
            path.to_string()
        } else {
            format!("{}/{}", self.base, path.trim_start_matches('/'))
        }
    }

    /// Only follow pagination links that stay on our API host.
    fn same_host(&self, url: &str) -> bool {
        url.starts_with(&format!("{}/", self.base)) || url == self.base
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.request("GET", path, "application/vnd.github+json", None)?;
        let mut response = check(response)?;
        response
            .body_mut()
            .read_json()
            .context("unexpected reply from GitHub")
    }

    /// The raw text of `path` with a custom media type (e.g. a diff).
    pub fn get_text(&self, path: &str, accept: &str) -> Result<String> {
        let response = self.request("GET", path, accept, None)?;
        let mut response = check(response)?;
        response
            .body_mut()
            .with_config()
            .limit(50 * 1024 * 1024)
            .read_to_string()
            .context("unexpected reply from GitHub")
    }

    /// Every page of a list (100 per page), up to [`MAX_ITEMS`].
    pub fn get_all<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        let separator = if path.contains('?') { '&' } else { '?' };
        let mut next = Some(format!("{path}{separator}per_page=100"));
        let mut items = Vec::new();
        while let Some(path) = next.take() {
            let response = self.request("GET", &path, "application/vnd.github+json", None)?;
            let mut response = check(response)?;
            next = response
                .headers()
                .get("link")
                .and_then(|link| link.to_str().ok())
                .and_then(next_link)
                .filter(|url| self.same_host(url));
            let page: Vec<T> = response
                .body_mut()
                .read_json()
                .context("unexpected reply from GitHub")?;
            items.extend(page);
            if items.len() >= MAX_ITEMS {
                items.truncate(MAX_ITEMS);
                break;
            }
        }
        Ok(items)
    }

    pub fn post<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        self.send("POST", path, body)
    }

    pub fn patch<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        self.send("PATCH", path, body)
    }

    pub fn put<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T> {
        self.send("PUT", path, body)
    }

    fn send<T: DeserializeOwned>(&self, method: &str, path: &str, body: &Value) -> Result<T> {
        let response = self.request(method, path, "application/vnd.github+json", Some(body))?;
        let mut response = check(response)?;
        response
            .body_mut()
            .read_json()
            .context("unexpected reply from GitHub")
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        accept: &str,
        body: Option<&Value>,
    ) -> Result<ureq::http::Response<ureq::Body>> {
        let url = self.url(path);
        if !self.same_host(&url) {
            bail!("refusing to send the token to another host: {url}");
        }
        let auth = format!("Bearer {}", self.token);
        macro_rules! with_headers {
            ($request:expr) => {
                $request
                    .header("Authorization", &auth)
                    .header("Accept", accept)
                    .header("X-GitHub-Api-Version", "2022-11-28")
            };
        }
        let result = match (method, body) {
            ("GET", _) => with_headers!(self.agent.get(&url)).call(),
            ("POST", Some(body)) => with_headers!(self.agent.post(&url)).send_json(body),
            ("PATCH", Some(body)) => with_headers!(self.agent.patch(&url)).send_json(body),
            ("PUT", Some(body)) => with_headers!(self.agent.put(&url)).send_json(body),
            ("DELETE", _) => with_headers!(self.agent.delete(&url)).call(),
            _ => bail!("unsupported request {method}"),
        };
        result.map_err(|err| anyhow!("could not reach {}: {err}", host_of(&self.base)))
    }
}

/// Turns error statuses into messages a person can act on.
fn check(
    mut response: ureq::http::Response<ureq::Body>,
) -> Result<ureq::http::Response<ureq::Body>> {
    let status = response.status().as_u16();
    if (200..300).contains(&status) {
        return Ok(response);
    }
    let remaining = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body: Value = response.body_mut().read_json().unwrap_or(Value::Null);
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let details: Vec<String> = body
        .get("errors")
        .and_then(Value::as_array)
        .map(|errors| {
            errors
                .iter()
                .filter_map(|error| {
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .or_else(|| error.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    let detail = if details.is_empty() {
        message.clone()
    } else {
        format!("{message}: {}", details.join("; "))
    };
    Err(match status {
        401 => anyhow!("GitHub rejected the token (401). Check it in Settings → GitHub."),
        403 if remaining.as_deref() == Some("0") => {
            anyhow!("GitHub's rate limit is used up. Try again later.")
        }
        403 if message.contains("SAML") => anyhow!(
            "This organization needs single sign-on: authorize the token for it on GitHub. ({message})"
        ),
        403 => anyhow!("GitHub refused (403): {detail}"),
        404 => anyhow!("Not found on GitHub (404), or the token cannot see it."),
        422 => anyhow!("GitHub could not do that (422): {detail}"),
        _ => anyhow!("GitHub returned {status}: {detail}"),
    })
}

/// The `rel="next"` URL of a `Link` header.
fn next_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let (url, rel) = part.split_once(';')?;
        rel.contains("rel=\"next\"").then(|| {
            url.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
    })
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_next_links() {
        let link = r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=5>; rel="last""#;
        assert_eq!(
            next_link(link).as_deref(),
            Some("https://api.github.com/x?page=2")
        );
        assert_eq!(next_link(r#"<https://a/x?page=1>; rel="prev""#), None);
    }

    #[test]
    fn never_sends_the_token_elsewhere() {
        let client = Client::new("https://github.example.com/api/v3", "secret");
        assert!(client.same_host("https://github.example.com/api/v3/repos/a/b"));
        assert!(!client.same_host("https://github.example.com.evil.test/api/v3/x"));
        assert!(!client.same_host("https://evil.test/"));
        assert!(client.get::<Value>("https://evil.test/steal").is_err());
    }
}
