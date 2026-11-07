//! Model prices for the usage estimate: what the tokens would cost at the
//! providers' API list prices, from LiteLLM's public price list
//! ([`SOURCE`]).
//!
//! PilotHouse ships a copy of that list (`prices.json`: Claude, GPT and
//! Gemini models, the four prices per model it needs) and never downloads
//! it by itself. *Update prices* in the usage dialog fetches the latest
//! list (a plain GET; nothing is sent) only when the user asks, and keeps
//! it in `<data dir>/model-prices.json`; a kept list newer than the shipped
//! one is used instead of it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// LiteLLM's list of model prices and context windows.
pub const SOURCE: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

/// The list shipped with this version of PilotHouse.
const SHIPPED: &str = include_str!("prices.json");

/// Where an updated list is kept.
fn kept_path() -> PathBuf {
    ph_store::paths::data_dir().join("model-prices.json")
}

/// One model's prices, in US dollars per token.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

impl Price {
    /// What `input` tokens (of which `cache_write` were written to a
    /// cache), `cached` read from one, and `output` cost.
    pub fn cost(&self, input: u64, cache_write: u64, cached: u64, output: u64) -> f64 {
        let cache_write = cache_write.min(input);
        (input - cache_write) as f64 * self.input
            + cache_write as f64 * self.cache_write.unwrap_or(self.input)
            + cached as f64 * self.cache_read.unwrap_or(self.input)
            + output as f64 * self.output
    }
}

/// The prices known, and when they were fetched.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Prices {
    /// Unix milliseconds; 0 when there are none.
    pub fetched_at: i64,
    pub models: HashMap<String, Price>,
}

impl Prices {
    /// The prices to estimate with: a list the user updated, when newer
    /// than the shipped one, else the shipped one. Reads no network.
    /// Blocking.
    pub fn load() -> Self {
        let kept: Option<Prices> = std::fs::read(kept_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        newest(Self::shipped(), kept)
    }

    fn shipped() -> Self {
        serde_json::from_str(SHIPPED).unwrap_or_default()
    }

    /// Downloads the latest list from [`SOURCE`] and keeps it, when the
    /// user asks (*Update prices*). Blocking.
    pub fn update(now_ms: i64) -> Option<Self> {
        let models = fetch().filter(|models| !models.is_empty())?;
        let prices = Prices {
            fetched_at: now_ms,
            models,
        };
        let path = kept_path();
        let bytes = serde_json::to_vec(&prices).ok()?;
        let partial = path.with_extension("json.part");
        std::fs::write(&partial, bytes).ok()?;
        std::fs::rename(&partial, &path).ok()?;
        Some(prices)
    }

    /// `model`'s prices: by its exact name, else without a date suffix
    /// (`-20251101`), with or without a provider's prefix.
    pub fn find(&self, model: &str) -> Option<Price> {
        let model = model.trim().to_lowercase();
        if model.is_empty() {
            return None;
        }
        let bare = model
            .rsplit_once('/')
            .map_or(model.as_str(), |(_, name)| name)
            .to_string();
        let undated = match bare.rsplit_once('-') {
            Some((name, date)) if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) => {
                name.to_string()
            }
            _ => bare.clone(),
        };
        // Cline writes some versions with a dot (`claude-sonnet-4.6`).
        let dashed = undated.replace('.', "-");
        [model.clone(), bare, undated, dashed]
            .into_iter()
            .flat_map(|name| {
                ["", "anthropic/", "openai/", "gemini/", "vertex_ai/"]
                    .map(|prefix| format!("{prefix}{name}"))
            })
            .find_map(|name| self.models.get(&name).copied())
    }
}

/// `kept` when it holds prices and is newer than `shipped`.
fn newest(shipped: Prices, kept: Option<Prices>) -> Prices {
    match kept {
        Some(kept) if !kept.models.is_empty() && kept.fetched_at > shipped.fetched_at => kept,
        _ => shipped,
    }
}

/// The list from [`SOURCE`], each chat model's prices.
fn fetch() -> Option<HashMap<String, Price>> {
    use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
    let tls = TlsConfig::builder()
        .provider(TlsProvider::NativeTls)
        .root_certs(RootCerts::PlatformVerifier)
        .build();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(concat!(ph_brand::stem!(), "/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let bytes = agent
        .get(SOURCE)
        .call()
        .ok()?
        .body_mut()
        .with_config()
        .limit(32 << 20)
        .read_to_vec()
        .ok()?;
    parse(&bytes)
}

/// Each model's input, output and cache prices from LiteLLM's list.
fn parse(bytes: &[u8]) -> Option<HashMap<String, Price>> {
    let list: HashMap<String, Value> = serde_json::from_slice(bytes).ok()?;
    let price = |entry: &Value, key: &str| entry.get(key).and_then(Value::as_f64);
    Some(
        list.into_iter()
            .filter_map(|(name, entry)| {
                Some((
                    name.to_lowercase(),
                    Price {
                        input: price(&entry, "input_cost_per_token")?,
                        output: price(&entry, "output_cost_per_token")?,
                        cache_read: price(&entry, "cache_read_input_token_cost"),
                        cache_write: price(&entry, "cache_creation_input_token_cost"),
                    },
                ))
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_list_and_finds_models_by_their_names() {
        let models = parse(
            br#"{
                "sample_spec": {"max_tokens": "set to max"},
                "claude-opus-5-5": {"input_cost_per_token": 0.000005, "output_cost_per_token": 0.000025,
                    "cache_read_input_token_cost": 0.0000005, "cache_creation_input_token_cost": 0.00000625},
                "openai/gpt-6-luna": {"input_cost_per_token": 0.000001, "output_cost_per_token": 0.000008},
                "gemini/gemini-3.8-flash": {"input_cost_per_token": 0.0000003, "output_cost_per_token": 0.0000025}
            }"#,
        )
        .unwrap();
        let prices = Prices {
            fetched_at: 1,
            models,
        };
        assert_eq!(prices.models.len(), 3, "no prices, no entry");
        assert!(
            prices.find("claude-opus-5-5-20261101").is_some(),
            "a dated name"
        );
        assert!(prices.find("gpt-6-luna").is_some(), "under its provider");
        assert!(prices.find("Gemini-3.8-Flash").is_some());
        assert!(prices.find("Opus 5.5").is_none());
        assert!(prices.find("").is_none());
        let opus = prices.find("claude-opus-5-5").unwrap();
        // 1M input of which 100k written to the cache, 2M read from it, 10k out.
        let cost = opus.cost(1_000_000, 100_000, 2_000_000, 10_000);
        let expected = 900_000. * 0.000005
            + 100_000. * 0.00000625
            + 2_000_000. * 0.0000005
            + 10_000. * 0.000025;
        assert!((cost - expected).abs() < 1e-9, "{cost} {expected}");
        // No cache prices: input's.
        let luna = prices.find("gpt-6-luna").unwrap();
        assert!((luna.cost(0, 0, 1_000, 0) - 0.001).abs() < 1e-12);
    }

    #[test]
    fn ships_prices_and_prefers_a_newer_update() {
        let shipped = Prices::shipped();
        assert!(shipped.fetched_at > 0);
        assert!(shipped.find("claude-opus-5-5").is_some(), "Claude");
        assert!(shipped.find("gpt-5.6").is_some(), "GPT");
        assert!(shipped.find("gemini-3.8-flash").is_some(), "Gemini");
        let older = Prices {
            fetched_at: shipped.fetched_at - 1,
            models: shipped.models.clone(),
        };
        let newer = Prices {
            fetched_at: shipped.fetched_at + 1,
            models: HashMap::from([("x".to_string(), shipped.find("gpt-5.6").unwrap())]),
        };
        let empty = Prices {
            fetched_at: shipped.fetched_at + 2,
            models: HashMap::new(),
        };
        assert_eq!(
            newest(shipped.clone(), Some(older)).fetched_at,
            shipped.fetched_at
        );
        assert_eq!(newest(shipped.clone(), Some(newer)).models.len(), 1);
        assert_eq!(
            newest(shipped.clone(), Some(empty)).fetched_at,
            shipped.fetched_at
        );
        assert_eq!(newest(shipped.clone(), None).fetched_at, shipped.fetched_at);
    }
}
