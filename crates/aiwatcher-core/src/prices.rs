//! What a model's tokens cost, as a person wrote it down from the provider.
//!
//! Nothing here is fetched. A provider's price page changes without notice and
//! a figure read from it yesterday is a claim about yesterday, so a price is an
//! entry a deployment loads, and every entry says where it was read and on
//! which day — the same rule the dataset sources table keeps for licences. A
//! cost computed from it carries those two facts beside the number, and a model
//! with no entry is counted as unpriced rather than priced at nought.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// One model's prices, per million tokens, in the table's currency.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    /// The name a call's `model` carries.
    pub model: String,
    pub input_per_million: f64,
    pub output_per_million: f64,
    /// What a cached input token costs, where the provider prices it apart.
    /// Absent, cached tokens are priced as input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input_per_million: Option<f64>,
    /// Where the figures were read: the provider's own page.
    pub source: String,
    /// The day they were read, `YYYY-MM-DD`.
    pub as_of: String,
}

/// A deployment's price table.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelPrices {
    /// One currency for the whole table, so no sum mixes two.
    pub currency: String,
    pub prices: Vec<ModelPrice>,
}

impl ModelPrices {
    /// Every problem with the table, at once.
    ///
    /// # Errors
    ///
    /// The sentences naming each entry that is not a price anybody can check.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        if self.currency.len() != 3 || !self.currency.bytes().all(|b| b.is_ascii_uppercase()) {
            problems.push(format!(
                "currency `{}` is not a three-letter code such as USD",
                self.currency
            ));
        }
        let mut seen = BTreeSet::new();
        for (at, price) in self.prices.iter().enumerate() {
            let named = if price.model.is_empty() {
                format!("price {at}")
            } else {
                price.model.clone()
            };
            if price.model.is_empty() {
                problems.push(format!("{named} names no model"));
            } else if !seen.insert(price.model.as_str()) {
                problems.push(format!("{named} is priced twice"));
            }
            for (field, value) in [
                ("input_per_million", Some(price.input_per_million)),
                ("output_per_million", Some(price.output_per_million)),
                ("cached_input_per_million", price.cached_input_per_million),
            ] {
                if value.is_some_and(|value| !value.is_finite() || value < 0.0) {
                    problems.push(format!(
                        "{named}: {field} is not an amount of nought or more"
                    ));
                }
            }
            if !(price.source.starts_with("https://") || price.source.starts_with("http://")) {
                problems.push(format!(
                    "{named}: source is not the address of the page the price was read from"
                ));
            }
            let date = price.as_of.as_bytes();
            let shaped = date.len() == 10
                && date[4] == b'-'
                && date[7] == b'-'
                && date
                    .iter()
                    .enumerate()
                    .all(|(at, byte)| at == 4 || at == 7 || byte.is_ascii_digit());
            if !shaped {
                problems.push(format!(
                    "{named}: as_of is not the day it was read, as YYYY-MM-DD"
                ));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }

    #[must_use]
    pub fn get(&self, model: &str) -> Option<&ModelPrice> {
        self.prices.iter().find(|price| price.model == model)
    }
}

/// Model calls that named one model, and the tokens they reported.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelUsage {
    pub model: String,
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    /// Of the input, how many a provider served from its cache.
    #[serde(default)]
    pub cached_tokens: i64,
}

impl ModelUsage {
    /// Add `usage` to the row for its model, keeping the rows ordered by model.
    pub fn add_to(rows: &mut Vec<Self>, usage: &Self) {
        match rows.binary_search_by(|row| row.model.cmp(&usage.model)) {
            Ok(at) => {
                let row = &mut rows[at];
                row.calls += usage.calls;
                row.input_tokens += usage.input_tokens;
                row.output_tokens += usage.output_tokens;
                row.cached_tokens += usage.cached_tokens;
            }
            Err(at) => rows.insert(at, usage.clone()),
        }
    }
}

/// What tokens cost at a deployment's price table, and what the figure rests on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TokenCost {
    pub currency: String,
    pub amount: f64,
    /// Calls whose model the table prices.
    pub priced_calls: u64,
    /// Calls whose model it does not, which cost something nobody priced —
    /// never nought.
    pub unpriced_calls: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unpriced_models: Vec<String>,
    /// Where each price used was read, and when.
    pub prices: Vec<PriceUsed>,
}

/// One price a cost used, with where and when it was read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PriceUsed {
    pub model: String,
    pub source: String,
    pub as_of: String,
}

impl ModelPrices {
    /// What this usage costs at this table.
    #[must_use]
    pub fn cost_of(&self, usage: &[ModelUsage]) -> TokenCost {
        let mut cost = TokenCost {
            currency: self.currency.clone(),
            amount: 0.0,
            priced_calls: 0,
            unpriced_calls: 0,
            unpriced_models: Vec::new(),
            prices: Vec::new(),
        };
        for model in usage {
            match self.get(&model.model) {
                Some(price) => {
                    cost.amount +=
                        price.cost(model.input_tokens, model.output_tokens, model.cached_tokens);
                    cost.priced_calls += model.calls;
                    if !cost.prices.iter().any(|used| used.model == price.model) {
                        cost.prices.push(PriceUsed {
                            model: price.model.clone(),
                            source: price.source.clone(),
                            as_of: price.as_of.clone(),
                        });
                    }
                }
                None => {
                    cost.unpriced_calls += model.calls;
                    if !cost.unpriced_models.contains(&model.model) {
                        cost.unpriced_models.push(model.model.clone());
                    }
                }
            }
        }
        cost
    }
}

impl ModelPrice {
    /// What these tokens cost. Cached tokens are counted inside input, as a
    /// provider reports them, and priced at the cached rate where there is one.
    #[must_use]
    pub fn cost(&self, input: i64, output: i64, cached: i64) -> f64 {
        let cached = cached.clamp(0, input.max(0));
        let fresh = (input.max(0) - cached) as f64;
        let cached_rate = self
            .cached_input_per_million
            .unwrap_or(self.input_per_million);
        (fresh * self.input_per_million
            + cached as f64 * cached_rate
            + output.max(0) as f64 * self.output_per_million)
            / 1_000_000.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price() -> ModelPrice {
        ModelPrice {
            model: "gpt-4o".into(),
            input_per_million: 2.5,
            output_per_million: 10.0,
            cached_input_per_million: Some(1.25),
            source: "https://openai.com/api/pricing".into(),
            as_of: "2026-09-01".into(),
        }
    }

    #[test]
    fn cached_input_is_priced_apart_inside_the_input_count() {
        // 1000 in, 400 of them cached, 100 out.
        let cost = price().cost(1_000, 100, 400);
        assert!((cost - (600.0 * 2.5 + 400.0 * 1.25 + 100.0 * 10.0) / 1e6).abs() < 1e-12);
    }

    #[test]
    fn a_price_without_where_and_when_it_was_read_is_refused() {
        let table = ModelPrices {
            currency: "usd".into(),
            prices: vec![
                price(),
                ModelPrice {
                    source: "their website".into(),
                    as_of: "last week".into(),
                    ..price()
                },
            ],
        };
        let problems = table.validate().expect_err("four problems");
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("priced twice"))
        );
        assert!(problems.iter().any(|problem| problem.contains("source")));
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("YYYY-MM-DD"))
        );
    }
}
