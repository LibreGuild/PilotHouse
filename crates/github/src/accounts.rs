//! GitHub accounts: one per host (github.com, GitHub Enterprise servers),
//! each with a personal access token kept in the OS credential store.

use anyhow::{Context as _, Result, bail};
use ph_store::Store;
use serde::{Deserialize, Serialize};

use crate::client::Client;
use crate::{pulls, secrets};

const KEY: &str = "github.accounts";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    /// `github.com` or an Enterprise host such as `github.example.net`.
    pub host: String,
    /// The user the token belongs to.
    pub login: String,
    /// A custom API root, for unusual Enterprise setups.
    #[serde(default)]
    pub api_url: Option<String>,
}

impl Account {
    /// `https://api.github.com`, or `https://HOST/api/v3` for Enterprise.
    pub fn api_base(&self) -> String {
        match &self.api_url {
            Some(url) if !url.trim().is_empty() => url.trim().trim_end_matches('/').to_string(),
            _ => api_base_for(&self.host),
        }
    }

    pub fn client(&self) -> Result<Client> {
        let token = secrets::get(&self.secret_name())?.with_context(|| {
            format!(
                "No token saved for {}. Add it again in Settings → GitHub.",
                self.host
            )
        })?;
        Ok(Client::new(&self.api_base(), &token))
    }

    fn secret_name(&self) -> String {
        format!("github:{}", self.id)
    }
}

pub fn api_base_for(host: &str) -> String {
    if host == "github.com" {
        "https://api.github.com".into()
    } else {
        format!("https://{host}/api/v3")
    }
}

/// `https://GitHub.Example.com/` → `github.example.com`.
pub fn normalize_host(input: &str) -> Result<String> {
    let host = input
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let valid = !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ':'))
        && !host.starts_with('.')
        && !host.starts_with('-');
    if !valid {
        bail!("`{input}` is not a host name");
    }
    Ok(host)
}

pub fn accounts(store: &Store) -> Vec<Account> {
    store.setting(KEY)
}

/// The account for `host`, if one is set up.
pub fn for_host(store: &Store, host: &str) -> Option<Account> {
    let host = host.to_ascii_lowercase();
    accounts(store)
        .into_iter()
        .find(|account| account.host == host)
}

/// Checks `token` against `host` (network: run off the UI thread), then
/// saves the account and its token. Replaces an account for the same host.
pub fn add(store: &Store, host: &str, token: &str, api_url: Option<String>) -> Result<Account> {
    let host = normalize_host(host)?;
    let token = token.trim();
    if token.is_empty() {
        bail!("Paste a personal access token.");
    }
    let api = api_url
        .clone()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| api_base_for(&host));
    let user = pulls::me(&Client::new(&api, token)).context("The token did not work")?;
    let account = Account {
        id: format!(
            "{}-{}",
            host.replace([':', '.'], "-"),
            user.login.to_ascii_lowercase()
        ),
        host,
        login: user.login,
        api_url,
    };
    secrets::set(&account.secret_name(), token)?;
    let mut list = accounts(store);
    list.retain(|known| known.host != account.host);
    list.push(account.clone());
    store.set_setting(KEY, &list)?;
    Ok(account)
}

pub fn remove(store: &Store, id: &str) -> Result<()> {
    let mut list = accounts(store);
    if let Some(account) = list.iter().find(|account| account.id == id) {
        secrets::delete(&account.secret_name())?;
    }
    list.retain(|account| account.id != id);
    store.set_setting(KEY, &list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_and_api_roots() {
        assert_eq!(normalize_host("https://GitHub.com/").unwrap(), "github.com");
        assert_eq!(
            normalize_host(" github.sec.example.net ").unwrap(),
            "github.sec.example.net"
        );
        assert!(normalize_host("bad host").is_err());
        assert!(normalize_host("").is_err());
        assert_eq!(api_base_for("github.com"), "https://api.github.com");
        assert_eq!(
            api_base_for("github.example.net"),
            "https://github.example.net/api/v3"
        );
        let custom = Account {
            id: "x".into(),
            host: "ghe".into(),
            login: "me".into(),
            api_url: Some("https://ghe/api/v3/".into()),
        };
        assert_eq!(custom.api_base(), "https://ghe/api/v3");
    }
}
