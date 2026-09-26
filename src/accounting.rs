use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

const NANOS_PER_UNIT: f64 = 1_000_000_000.0;
const MULTIPLIER_SCALE: f64 = 1_000_000.0;

fn default_unit_tokens() -> u64 {
    1_000_000
}

fn default_currency() -> String {
    "USD".to_string()
}

fn default_applies_above_threshold() -> bool {
    true
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PricingConfig {
    pub applies_to_models: Vec<String>,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default = "default_unit_tokens")]
    pub unit_tokens: u64,
    pub effective_as_of: String,
    #[serde(default)]
    pub valid_through: Option<String>,
    pub provenance: PricingProvenance,
    pub rates: PricingRates,
    #[serde(default)]
    pub long_context: Option<LongContextPricing>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PricingProvenance {
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PricingRates {
    pub uncached_input: f64,
    pub cached_read_input: f64,
    pub cache_creation_input: f64,
    pub output: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LongContextPricing {
    pub threshold_input_tokens: u64,
    #[serde(default = "default_applies_above_threshold")]
    pub applies_above_threshold: bool,
    pub input_multiplier: f64,
    pub output_multiplier: f64,
}

impl PricingConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.applies_to_models.is_empty() {
            return Err("pricing.applies_to_models cannot be empty".to_string());
        }
        let mut models = std::collections::HashSet::new();
        for model in &self.applies_to_models {
            if model.trim().is_empty() {
                return Err("pricing.applies_to_models cannot contain an empty model".to_string());
            }
            if !models.insert(model) {
                return Err(format!("pricing contains duplicate model '{model}'"));
            }
        }
        if self.currency.trim().is_empty()
            || !self
                .currency
                .chars()
                .all(|character| character.is_ascii_uppercase())
        {
            return Err(
                "pricing.currency must be a nonempty uppercase unit such as USD".to_string(),
            );
        }
        if self.unit_tokens == 0 {
            return Err("pricing.unit_tokens must be greater than zero".to_string());
        }
        parse_date(&self.effective_as_of, "pricing.effective_as_of")?;
        if let Some(valid_through) = &self.valid_through {
            let effective = parse_date(&self.effective_as_of, "pricing.effective_as_of")?;
            let valid = parse_date(valid_through, "pricing.valid_through")?;
            if valid < effective {
                return Err("pricing.valid_through cannot precede effective_as_of".to_string());
            }
        }
        if self.provenance.kind.trim().is_empty() {
            return Err("pricing.provenance.kind cannot be empty".to_string());
        }
        let url = reqwest::Url::parse(&self.provenance.url)
            .map_err(|error| format!("pricing.provenance.url is invalid: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("pricing.provenance.url must use http or https".to_string());
        }
        for (name, value) in [
            ("uncached_input", self.rates.uncached_input),
            ("cached_read_input", self.rates.cached_read_input),
            ("cache_creation_input", self.rates.cache_creation_input),
            ("output", self.rates.output),
        ] {
            validate_nonnegative_finite(value, &format!("pricing.rates.{name}"))?;
            scaled(value, NANOS_PER_UNIT, &format!("pricing.rates.{name}"))?;
        }
        if let Some(long) = &self.long_context {
            if long.threshold_input_tokens == 0 {
                return Err(
                    "pricing.long_context.threshold_input_tokens must be greater than zero"
                        .to_string(),
                );
            }
            validate_nonnegative_finite(
                long.input_multiplier,
                "pricing.long_context.input_multiplier",
            )?;
            validate_nonnegative_finite(
                long.output_multiplier,
                "pricing.long_context.output_multiplier",
            )?;
            scaled(
                long.input_multiplier,
                MULTIPLIER_SCALE,
                "pricing.long_context.input_multiplier",
            )?;
            scaled(
                long.output_multiplier,
                MULTIPLIER_SCALE,
                "pricing.long_context.output_multiplier",
            )?;
        }
        Ok(())
    }

    pub fn applies_to(&self, model: &str) -> bool {
        self.applies_to_models
            .iter()
            .any(|candidate| candidate == model)
    }

    pub fn is_stale_on(&self, date: NaiveDate) -> bool {
        self.valid_through
            .as_deref()
            .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
            .is_some_and(|valid_through| date > valid_through)
    }

    pub fn estimate(&self, model: &str, usage: &Usage) -> Result<Option<EstimatedCost>, String> {
        self.validate()?;
        if !self.applies_to(model) {
            return Ok(None);
        }

        let total_input = usage.total_input();
        let (input_multiplier, output_multiplier, long_context_applied) = self
            .long_context
            .as_ref()
            .filter(|tier| {
                if tier.applies_above_threshold {
                    total_input > tier.threshold_input_tokens
                } else {
                    total_input >= tier.threshold_input_tokens
                }
            })
            .map(|tier| (tier.input_multiplier, tier.output_multiplier, true))
            .unwrap_or((1.0, 1.0, false));

        let input_multiplier = scaled(
            input_multiplier,
            MULTIPLIER_SCALE,
            "pricing input multiplier",
        )?;
        let output_multiplier = scaled(
            output_multiplier,
            MULTIPLIER_SCALE,
            "pricing output multiplier",
        )?;
        let denominator = u128::from(self.unit_tokens)
            .checked_mul(MULTIPLIER_SCALE as u128)
            .ok_or_else(|| "pricing denominator overflowed".to_string())?;

        let components = [
            (
                usage.uncached_input_tokens,
                self.rates.uncached_input,
                input_multiplier,
                "uncached input",
            ),
            (
                usage.cache_read_input_tokens,
                self.rates.cached_read_input,
                input_multiplier,
                "cache-read input",
            ),
            (
                usage.cache_creation_input_tokens,
                self.rates.cache_creation_input,
                input_multiplier,
                "cache-creation input",
            ),
            (
                usage.output_tokens,
                self.rates.output,
                output_multiplier,
                "output",
            ),
        ];
        let mut nanos = 0_u128;
        for (tokens, rate, multiplier, label) in components {
            let rate = scaled(rate, NANOS_PER_UNIT, &format!("{label} rate"))?;
            let numerator = u128::from(tokens)
                .checked_mul(rate)
                .and_then(|value| value.checked_mul(multiplier))
                .ok_or_else(|| format!("{label} cost overflowed"))?;
            let rounded = numerator
                .checked_add(denominator / 2)
                .ok_or_else(|| format!("{label} rounding overflowed"))?
                / denominator;
            nanos = nanos
                .checked_add(rounded)
                .ok_or_else(|| "estimated cost overflowed".to_string())?;
        }
        let nanos = u64::try_from(nanos)
            .map_err(|_| "estimated cost exceeded the supported range".to_string())?;
        Ok(Some(EstimatedCost {
            currency: self.currency.clone(),
            nanos,
            incomplete: !usage.breakdown_complete,
            mixed_pricing: false,
            long_context_applied,
            pricing_effective_as_of: self.effective_as_of.clone(),
            pricing_valid_through: self.valid_through.clone(),
            provenance_kind: self.provenance.kind.clone(),
        }))
    }
}

fn parse_date(value: &str, field: &str) -> Result<NaiveDate, String> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map_err(|error| format!("{field} must use YYYY-MM-DD: {error}"))
}

fn validate_nonnegative_finite(value: f64, field: &str) -> Result<(), String> {
    if !value.is_finite() || value < 0.0 {
        return Err(format!("{field} must be finite and nonnegative"));
    }
    Ok(())
}

fn scaled(value: f64, scale: f64, field: &str) -> Result<u128, String> {
    validate_nonnegative_finite(value, field)?;
    let scaled = value * scale;
    if !scaled.is_finite() || scaled > u128::MAX as f64 {
        return Err(format!("{field} exceeds the supported range"));
    }
    Ok(scaled.round() as u128)
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    #[serde(default)]
    pub uncached_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub total_input_tokens: Option<u64>,
    #[serde(default)]
    pub breakdown_complete: bool,
}

impl Usage {
    pub fn from_legacy_counts(
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
    ) -> Option<Self> {
        if input_tokens.is_none() && output_tokens.is_none() {
            return None;
        }
        let input_tokens = input_tokens.unwrap_or(0);
        Some(Self {
            uncached_input_tokens: input_tokens,
            output_tokens: output_tokens.unwrap_or(0),
            total_input_tokens: Some(input_tokens),
            breakdown_complete: false,
            ..Self::default()
        })
    }

    pub fn total_input(&self) -> u64 {
        self.total_input_tokens.unwrap_or_else(|| {
            self.uncached_input_tokens
                .saturating_add(self.cache_read_input_tokens)
                .saturating_add(self.cache_creation_input_tokens)
        })
    }

    pub fn total_tokens(&self) -> u64 {
        self.total_input().saturating_add(self.output_tokens)
    }

    pub fn saturating_add(self, other: Self) -> Self {
        let component_total = self.total_input().saturating_add(other.total_input());
        Self {
            uncached_input_tokens: self
                .uncached_input_tokens
                .saturating_add(other.uncached_input_tokens),
            cache_read_input_tokens: self
                .cache_read_input_tokens
                .saturating_add(other.cache_read_input_tokens),
            cache_creation_input_tokens: self
                .cache_creation_input_tokens
                .saturating_add(other.cache_creation_input_tokens),
            output_tokens: self.output_tokens.saturating_add(other.output_tokens),
            total_input_tokens: Some(component_total),
            breakdown_complete: self.breakdown_complete && other.breakdown_complete,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EstimatedCost {
    pub currency: String,
    /// Billionths of the configured currency unit.
    pub nanos: u64,
    pub incomplete: bool,
    #[serde(default)]
    pub mixed_pricing: bool,
    pub long_context_applied: bool,
    pub pricing_effective_as_of: String,
    #[serde(default)]
    pub pricing_valid_through: Option<String>,
    pub provenance_kind: String,
}

impl EstimatedCost {
    pub fn amount(&self) -> f64 {
        self.nanos as f64 / NANOS_PER_UNIT
    }

    pub fn is_stale_on(&self, date: NaiveDate) -> bool {
        self.pricing_valid_through
            .as_deref()
            .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
            .is_some_and(|valid_through| date > valid_through)
    }

    pub fn saturating_add(&self, other: &Self) -> Option<Self> {
        if self.currency != other.currency {
            return None;
        }
        let mixed_pricing = self.mixed_pricing
            || other.mixed_pricing
            || self.pricing_effective_as_of != other.pricing_effective_as_of
            || self.provenance_kind != other.provenance_kind
            || self.pricing_valid_through != other.pricing_valid_through;
        Some(Self {
            currency: self.currency.clone(),
            nanos: self.nanos.saturating_add(other.nanos),
            incomplete: self.incomplete || other.incomplete,
            mixed_pricing,
            long_context_applied: self.long_context_applied || other.long_context_applied,
            pricing_effective_as_of: self
                .pricing_effective_as_of
                .clone()
                .min(other.pricing_effective_as_of.clone()),
            pricing_valid_through: match (&self.pricing_valid_through, &other.pricing_valid_through)
            {
                (Some(left), Some(right)) => Some(left.min(right).clone()),
                (Some(value), None) | (None, Some(value)) => Some(value.clone()),
                (None, None) => None,
            },
            provenance_kind: if self.provenance_kind == other.provenance_kind {
                self.provenance_kind.clone()
            } else {
                "mixed".to_string()
            },
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProviderRequestAccounting {
    pub request_id: String,
    pub connection_id: String,
    pub model: String,
    pub usage: Usage,
    #[serde(default)]
    pub usage_reported: bool,
    #[serde(default)]
    pub estimated_cost: Option<EstimatedCost>,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub in_flight: bool,
}

impl ProviderRequestAccounting {
    pub fn into_logical_turn(self, logical_turn_id: String) -> RequestAccounting {
        RequestAccounting {
            request_id: self.request_id,
            logical_turn_id,
            connection_id: self.connection_id,
            model: self.model,
            usage: self.usage,
            estimated_cost: self.estimated_cost,
            completed: self.completed,
            in_flight: self.in_flight,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequestAccounting {
    pub request_id: String,
    #[serde(default)]
    pub logical_turn_id: String,
    pub connection_id: String,
    pub model: String,
    pub usage: Usage,
    #[serde(default)]
    pub estimated_cost: Option<EstimatedCost>,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub in_flight: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AccountingTotals {
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub estimated_cost: Option<EstimatedCost>,
    #[serde(default)]
    pub request_count: u64,
    #[serde(default)]
    pub long_context_request_count: u64,
    #[serde(default)]
    pub unpriced_request_count: u64,
    #[serde(default)]
    pub incomplete_usage_request_count: u64,
}

impl Default for AccountingTotals {
    fn default() -> Self {
        Self {
            usage: Usage {
                breakdown_complete: true,
                ..Usage::default()
            },
            estimated_cost: None,
            request_count: 0,
            long_context_request_count: 0,
            unpriced_request_count: 0,
            incomplete_usage_request_count: 0,
        }
    }
}

impl AccountingTotals {
    pub fn record(&mut self, request: &RequestAccounting) {
        self.request_count = self.request_count.saturating_add(1);
        self.usage = self.usage.saturating_add(request.usage);
        if !request.usage.breakdown_complete {
            self.incomplete_usage_request_count =
                self.incomplete_usage_request_count.saturating_add(1);
        }
        match &request.estimated_cost {
            Some(cost) => {
                if cost.long_context_applied {
                    self.long_context_request_count =
                        self.long_context_request_count.saturating_add(1);
                }
                self.estimated_cost = match &self.estimated_cost {
                    Some(total) => match total.saturating_add(cost) {
                        Some(sum) => Some(sum),
                        None => {
                            self.unpriced_request_count =
                                self.unpriced_request_count.saturating_add(1);
                            Some(total.clone())
                        }
                    },
                    None => Some(cost.clone()),
                };
            }
            None => {
                self.unpriced_request_count = self.unpriced_request_count.saturating_add(1);
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct SessionAccounting {
    #[serde(default)]
    pub requests: Vec<RequestAccounting>,
    #[serde(default)]
    pub latest_logical_turn_id: Option<String>,
    #[serde(default)]
    pub latest_logical_turn: AccountingTotals,
    #[serde(default)]
    pub session: AccountingTotals,
}

impl SessionAccounting {
    /// Records a provider request exactly once. Replaying the same entry is a no-op;
    /// reusing an ID for different accounting data is rejected.
    pub fn record_request(&mut self, request: RequestAccounting) -> Result<bool, String> {
        if request.request_id.trim().is_empty() {
            return Err("accounting request_id cannot be empty".to_string());
        }
        if request.logical_turn_id.trim().is_empty() {
            return Err("accounting logical_turn_id cannot be empty".to_string());
        }
        if request.in_flight && (request.completed || request.estimated_cost.is_some()) {
            return Err("in-flight accounting cannot be completed or priced".to_string());
        }
        if let Some(index) = self
            .requests
            .iter()
            .position(|existing| existing.request_id == request.request_id)
        {
            let existing = &self.requests[index];
            if existing == &request {
                return Ok(false);
            }
            if existing.in_flight
                && !request.in_flight
                && existing.logical_turn_id == request.logical_turn_id
                && existing.connection_id == request.connection_id
                && existing.model == request.model
            {
                self.requests[index] = request;
                self.rebuild_totals()?;
                return Ok(true);
            }
            return Err(format!(
                "accounting request_id '{}' was reused with different data",
                request.request_id
            ));
        }

        if self.latest_logical_turn_id.as_deref() != Some(request.logical_turn_id.as_str()) {
            self.latest_logical_turn_id = Some(request.logical_turn_id.clone());
            self.latest_logical_turn = AccountingTotals::default();
        }
        self.latest_logical_turn.record(&request);
        self.session.record(&request);
        self.requests.push(request);
        Ok(true)
    }

    /// Records a request that belongs to an older logical turn without changing
    /// which turn is presented as the latest one.
    pub fn record_request_preserving_latest(
        &mut self,
        request: RequestAccounting,
    ) -> Result<bool, String> {
        if request.request_id.trim().is_empty() {
            return Err("accounting request_id cannot be empty".to_string());
        }
        if request.logical_turn_id.trim().is_empty() {
            return Err("accounting logical_turn_id cannot be empty".to_string());
        }
        if request.in_flight && (request.completed || request.estimated_cost.is_some()) {
            return Err("in-flight accounting cannot be completed or priced".to_string());
        }
        if let Some(index) = self
            .requests
            .iter()
            .position(|existing| existing.request_id == request.request_id)
        {
            let existing = &self.requests[index];
            if existing == &request {
                return Ok(false);
            }
            if existing.in_flight
                && !request.in_flight
                && existing.logical_turn_id == request.logical_turn_id
                && existing.connection_id == request.connection_id
                && existing.model == request.model
            {
                self.requests[index] = request;
                self.rebuild_totals()?;
                return Ok(true);
            }
            return Err(format!(
                "accounting request_id '{}' was reused with different data",
                request.request_id
            ));
        }
        if self.latest_logical_turn_id.as_deref() == Some(request.logical_turn_id.as_str()) {
            self.latest_logical_turn.record(&request);
        }
        self.session.record(&request);
        self.requests.push(request);
        Ok(true)
    }

    pub fn totals_for_logical_turn(&self, logical_turn_id: &str) -> AccountingTotals {
        let mut totals = AccountingTotals::default();
        for request in self
            .requests
            .iter()
            .filter(|request| request.logical_turn_id == logical_turn_id)
        {
            totals.record(request);
        }
        totals
    }

    /// Rebuilds derived totals after loading persisted request entries.
    pub fn rebuild_totals(&mut self) -> Result<(), String> {
        let saved_latest = self.latest_logical_turn_id.clone();
        let mut rebuilt = Self::default();
        for request in self.requests.clone() {
            rebuilt.record_request(request)?;
        }
        if let Some(saved_latest) = saved_latest {
            if !rebuilt
                .requests
                .iter()
                .any(|request| request.logical_turn_id == saved_latest)
            {
                return Err(format!(
                    "accounting latest logical turn '{saved_latest}' has no request entry"
                ));
            }
            rebuilt.latest_logical_turn = rebuilt.totals_for_logical_turn(&saved_latest);
            rebuilt.latest_logical_turn_id = Some(saved_latest);
        }
        *self = rebuilt;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sol_pricing() -> PricingConfig {
        PricingConfig {
            applies_to_models: vec!["gpt-5.6-sol".to_string()],
            currency: "USD".to_string(),
            unit_tokens: 1_000_000,
            effective_as_of: "2026-08-25".to_string(),
            valid_through: Some("2026-11-21".to_string()),
            provenance: PricingProvenance {
                kind: "api_equivalent_estimate".to_string(),
                url: "https://developers.openai.com/api/docs/models/gpt-5.6-sol".to_string(),
                note: Some("Subscription billing may differ.".to_string()),
            },
            rates: PricingRates {
                uncached_input: 4.0,
                cached_read_input: 0.4,
                cache_creation_input: 5.0,
                output: 20.0,
            },
            long_context: Some(LongContextPricing {
                threshold_input_tokens: 272_000,
                applies_above_threshold: true,
                input_multiplier: 2.0,
                output_multiplier: 1.5,
            }),
        }
    }

    #[test]
    fn cache_aware_sol_estimate_matches_published_rates() {
        let usage = Usage {
            uncached_input_tokens: 10_000,
            cache_read_input_tokens: 90_000,
            output_tokens: 10_000,
            total_input_tokens: Some(100_000),
            breakdown_complete: true,
            ..Usage::default()
        };
        let cost = sol_pricing()
            .estimate("gpt-5.6-sol", &usage)
            .unwrap()
            .unwrap();
        assert_eq!(cost.nanos, 276_000_000);
        assert!((cost.amount() - 0.276).abs() < 1e-12);
        assert!(!cost.long_context_applied);
    }

    #[test]
    fn long_context_multiplier_applies_only_above_threshold() {
        let pricing = sol_pricing();
        let at = Usage {
            uncached_input_tokens: 272_000,
            output_tokens: 10_000,
            total_input_tokens: Some(272_000),
            breakdown_complete: true,
            ..Usage::default()
        };
        let above = Usage {
            uncached_input_tokens: 272_001,
            output_tokens: 10_000,
            total_input_tokens: Some(272_001),
            breakdown_complete: true,
            ..Usage::default()
        };
        let at_cost = pricing.estimate("gpt-5.6-sol", &at).unwrap().unwrap();
        let above_cost = pricing.estimate("gpt-5.6-sol", &above).unwrap().unwrap();
        assert!(!at_cost.long_context_applied);
        assert!(above_cost.long_context_applied);
        assert_eq!(at_cost.nanos, 1_288_000_000);
        assert_eq!(above_cost.nanos, 2_476_008_000);
    }

    #[test]
    fn model_scope_and_dates_are_validated() {
        let pricing = sol_pricing();
        assert!(pricing.validate().is_ok());
        assert!(
            pricing
                .estimate("another-model", &Usage::default())
                .unwrap()
                .is_none()
        );
        assert!(!pricing.is_stale_on(NaiveDate::from_ymd_opt(2026, 11, 21).unwrap()));
        assert!(pricing.is_stale_on(NaiveDate::from_ymd_opt(2026, 11, 22).unwrap()));
    }

    #[test]
    fn totals_accumulate_each_request_after_per_request_tiering() {
        let pricing = sol_pricing();
        let usage = Usage {
            uncached_input_tokens: 272_001,
            output_tokens: 1,
            total_input_tokens: Some(272_001),
            breakdown_complete: true,
            ..Usage::default()
        };
        let cost = pricing.estimate("gpt-5.6-sol", &usage).unwrap();
        let request = RequestAccounting {
            request_id: "one".to_string(),
            logical_turn_id: "turn-one".to_string(),
            connection_id: "proxy".to_string(),
            model: "gpt-5.6-sol".to_string(),
            usage,
            estimated_cost: cost,
            completed: true,
            in_flight: false,
        };
        let mut totals = AccountingTotals::default();
        totals.record(&request);
        totals.record(&RequestAccounting {
            request_id: "two".to_string(),
            ..request
        });
        assert_eq!(totals.request_count, 2);
        assert_eq!(totals.long_context_request_count, 2);
        assert_eq!(totals.estimated_cost.unwrap().nanos, 4_352_076_000);
    }

    #[test]
    fn session_cost_sums_compatible_currency_across_pricing_snapshots() {
        let usage = Usage {
            uncached_input_tokens: 1_000,
            output_tokens: 100,
            total_input_tokens: Some(1_000),
            breakdown_complete: true,
            ..Usage::default()
        };
        let first = sol_pricing()
            .estimate("gpt-5.6-sol", &usage)
            .unwrap()
            .unwrap();
        let mut second = first.clone();
        second.pricing_effective_as_of = "2026-09-01".to_string();
        second.provenance_kind = "another_estimate".to_string();
        let sum = first.saturating_add(&second).unwrap();
        assert_eq!(sum.nanos, first.nanos.saturating_mul(2));
        assert!(sum.mixed_pricing);
        assert_eq!(sum.provenance_kind, "mixed");

        let mut incompatible = second;
        incompatible.currency = "EUR".to_string();
        assert!(first.saturating_add(&incompatible).is_none());
    }

    #[test]
    fn in_flight_request_is_durably_upgraded_in_place() {
        let mut ledger = SessionAccounting::default();
        let provisional = RequestAccounting {
            request_id: "request-one".to_string(),
            logical_turn_id: "turn-one".to_string(),
            connection_id: "proxy".to_string(),
            model: "gpt-5.6-sol".to_string(),
            usage: Usage::default(),
            estimated_cost: None,
            completed: false,
            in_flight: true,
        };
        ledger.record_request(provisional.clone()).unwrap();
        assert_eq!(ledger.requests.len(), 1);
        assert_eq!(ledger.session.incomplete_usage_request_count, 1);

        let mut finished = provisional;
        finished.in_flight = false;
        finished.completed = true;
        finished.usage = Usage {
            uncached_input_tokens: 42,
            total_input_tokens: Some(42),
            breakdown_complete: true,
            ..Usage::default()
        };
        ledger.record_request(finished).unwrap();

        assert_eq!(ledger.requests.len(), 1);
        assert!(!ledger.requests[0].in_flight);
        assert_eq!(ledger.session.usage.total_input(), 42);
        assert_eq!(ledger.session.incomplete_usage_request_count, 0);
    }

    #[test]
    fn late_request_can_update_old_turn_without_rebinding_latest_turn() {
        let request = |request_id: &str, turn: &str, input: u64| RequestAccounting {
            request_id: request_id.to_string(),
            logical_turn_id: turn.to_string(),
            connection_id: "proxy".to_string(),
            model: "gpt-5.6-sol".to_string(),
            usage: Usage {
                uncached_input_tokens: input,
                total_input_tokens: Some(input),
                breakdown_complete: true,
                ..Usage::default()
            },
            estimated_cost: None,
            completed: true,
            in_flight: false,
        };
        let mut ledger = SessionAccounting::default();
        ledger
            .record_request(request("one", "turn-one", 10))
            .unwrap();
        ledger
            .record_request(request("two", "turn-two", 20))
            .unwrap();
        ledger
            .record_request_preserving_latest(request("late", "turn-one", 30))
            .unwrap();

        assert_eq!(ledger.latest_logical_turn_id.as_deref(), Some("turn-two"));
        assert_eq!(ledger.latest_logical_turn.usage.total_input(), 20);
        assert_eq!(
            ledger
                .totals_for_logical_turn("turn-one")
                .usage
                .total_input(),
            40
        );
        assert_eq!(ledger.session.usage.total_input(), 60);

        ledger.latest_logical_turn = AccountingTotals::default();
        ledger.session = AccountingTotals::default();
        ledger.rebuild_totals().unwrap();
        assert_eq!(ledger.latest_logical_turn_id.as_deref(), Some("turn-two"));
        assert_eq!(ledger.latest_logical_turn.usage.total_input(), 20);
        assert_eq!(ledger.session.usage.total_input(), 60);
    }

    #[test]
    fn session_ledger_accumulates_tool_continuations_idempotently() {
        let pricing = sol_pricing();
        let make_request = |request_id: &str, turn_id: &str, input: u64| {
            let usage = Usage {
                uncached_input_tokens: input,
                output_tokens: 10,
                total_input_tokens: Some(input),
                breakdown_complete: true,
                ..Usage::default()
            };
            RequestAccounting {
                request_id: request_id.to_string(),
                logical_turn_id: turn_id.to_string(),
                connection_id: "proxy".to_string(),
                model: "gpt-5.6-sol".to_string(),
                usage,
                estimated_cost: pricing.estimate("gpt-5.6-sol", &usage).unwrap(),
                completed: true,
                in_flight: false,
            }
        };

        let first = make_request("request-1", "turn-1", 100);
        let continuation = make_request("request-2", "turn-1", 200);
        let next_turn = make_request("request-3", "turn-2", 300);
        let mut ledger = SessionAccounting::default();

        assert!(ledger.record_request(first.clone()).unwrap());
        assert!(ledger.record_request(continuation).unwrap());
        assert!(!ledger.record_request(first).unwrap());
        assert_eq!(ledger.latest_logical_turn.request_count, 2);
        assert_eq!(ledger.latest_logical_turn.usage.total_input(), 300);
        assert_eq!(ledger.session.request_count, 2);

        assert!(ledger.record_request(next_turn).unwrap());
        assert_eq!(ledger.latest_logical_turn_id.as_deref(), Some("turn-2"));
        assert_eq!(ledger.latest_logical_turn.request_count, 1);
        assert_eq!(ledger.latest_logical_turn.usage.total_input(), 300);
        assert_eq!(ledger.session.request_count, 3);
        assert_eq!(ledger.session.usage.total_input(), 600);

        let mut conflicting = ledger.requests[0].clone();
        conflicting.model = "different".to_string();
        assert!(
            ledger
                .record_request(conflicting)
                .unwrap_err()
                .contains("reused")
        );

        ledger.latest_logical_turn = AccountingTotals::default();
        ledger.session = AccountingTotals::default();
        ledger.rebuild_totals().unwrap();
        assert_eq!(ledger.latest_logical_turn.request_count, 1);
        assert_eq!(ledger.session.request_count, 3);
    }
}
