//! Models the user added to a connection from the model picker's catalog
//! scan, stored per user in `~/.config/lethetic/saved_models.yml` and merged
//! into each connection's `models` allowlist at startup. Keeping them in a
//! separate file means lethetic never rewrites the hand-edited config.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::accounting::PricingConfig;
use crate::config::Config;
use crate::transport::CatalogPricing;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedModels {
    /// Connection id → model ids, in the order they were added.
    #[serde(default)]
    pub connections: BTreeMap<String, Vec<String>>,
    /// Connection id → model id → list price captured from the catalog.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pricing: BTreeMap<String, BTreeMap<String, PricingConfig>>,
}

impl SavedModels {
    pub fn path() -> PathBuf {
        crate::platform::lethetic_config_dir().join("saved_models.yml")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_yaml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        }
        let text = serde_yaml::to_string(self).map_err(|error| error.to_string())?;
        std::fs::write(&path, text)
            .map_err(|error| format!("could not write {}: {error}", path.display()))
    }

    /// Record `model` for `connection_id`. Returns false when already saved.
    pub fn add(&mut self, connection_id: &str, model: &str) -> bool {
        let models = self
            .connections
            .entry(connection_id.to_string())
            .or_default();
        if models.iter().any(|existing| existing == model) {
            return false;
        }
        models.push(model.to_string());
        true
    }

    /// Merge saved models into each matching connection's allowlist. A
    /// connection without an allowlist first gets its default model, so
    /// adding a model never hides the one already configured.
    pub fn apply(&self, config: &mut Config) {
        for server in &mut config.model_servers {
            if let Some(prices) = self.pricing.get(server.connection_id()) {
                server.model_pricing.extend(prices.values().cloned());
            }
            let Some(saved) = self.connections.get(server.connection_id()) else {
                continue;
            };
            add_to_allowlist(&mut server.models, &server.model, saved);
        }
    }

    /// Remember the catalog price of `model`. Returns false if the catalog
    /// published no usable price.
    pub fn record_price(
        &mut self,
        connection_id: &str,
        model: &str,
        catalog: &CatalogPricing,
        source_url: &str,
    ) -> bool {
        let Some(pricing) = pricing_from_catalog(model, catalog, source_url) else {
            return false;
        };
        self.pricing
            .entry(connection_id.to_string())
            .or_default()
            .insert(model.to_string(), pricing);
        true
    }
}

/// Convert per-token list prices into a per-million `PricingConfig` that
/// applies to exactly `model`.
pub fn pricing_from_catalog(
    model: &str,
    catalog: &CatalogPricing,
    source_url: &str,
) -> Option<PricingConfig> {
    let per_million = |value: &str| {
        value
            .parse::<f64>()
            .ok()
            .filter(|rate| rate.is_finite() && *rate >= 0.0)
            .map(|rate| rate * 1_000_000.0)
    };
    let input = per_million(&catalog.prompt)?;
    let output = per_million(&catalog.completion)?;
    let cached_read = catalog
        .cache_read
        .as_deref()
        .and_then(per_million)
        .unwrap_or(input);
    let cache_creation = catalog
        .cache_write
        .as_deref()
        .and_then(per_million)
        .unwrap_or(input);
    let yaml = format!(
        "applies_to_models: [{model:?}]\ncurrency: USD\nunit_tokens: 1000000\neffective_as_of: {today}\nprovenance:\n  kind: catalog_list_price\n  url: {source_url:?}\n  note: Captured from the provider's model catalog; refresh by scanning again.\nrates:\n  uncached_input: {input}\n  cached_read_input: {cached_read}\n  cache_creation_input: {cache_creation}\n  output: {output}\n",
        today = chrono::Local::now().format("%Y-%m-%d"),
    );
    let pricing: PricingConfig = serde_yaml::from_str(&yaml).ok()?;
    pricing.validate().ok()?;
    Some(pricing)
}

pub fn add_to_allowlist(allowlist: &mut Vec<String>, default_model: &str, models: &[String]) {
    if allowlist.is_empty() {
        allowlist.push(default_model.to_string());
    }
    for model in models {
        if !allowlist.contains(model) {
            allowlist.push(model.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_keeps_the_default_model_and_ignores_duplicates() {
        let mut allowlist = Vec::new();
        add_to_allowlist(
            &mut allowlist,
            "muse",
            &["glm".to_string(), "muse".to_string()],
        );
        assert_eq!(allowlist, ["muse", "glm"]);

        let mut saved = SavedModels::default();
        assert!(saved.add("openrouter", "glm"));
        assert!(!saved.add("openrouter", "glm"));
        let price = CatalogPricing {
            prompt: "0.0000001".into(),
            completion: "0.0000002".into(),
            cache_read: Some("0.000000002".into()),
            cache_write: None,
        };
        assert!(saved.record_price("openrouter", "glm", &price, "https://openrouter.ai/glm"));
        let pricing = &saved.pricing["openrouter"]["glm"];
        assert!(pricing.applies_to("glm"));
        assert!((pricing.rates.output - 0.2).abs() < 1e-9);
        let text = serde_yaml::to_string(&saved).unwrap();
        assert_eq!(serde_yaml::from_str::<SavedModels>(&text).unwrap(), saved);
    }
}
