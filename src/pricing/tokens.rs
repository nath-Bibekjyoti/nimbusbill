use crate::db::Database;
use crate::models::TokenUsageSpec;
use anyhow::{Context, Result};
use rust_decimal::Decimal;

pub fn estimate_token_cost(db: &Database, usage: &TokenUsageSpec) -> Result<Decimal> {
    let provider = usage
        .cloud()
        .map(|p| p.as_str())
        .unwrap_or(usage.provider.as_str());
    let row = db.lookup_token_price(provider, &usage.model)?;
    let (input_per_mtok, output_per_mtok) = match row {
        Some(v) => v,
        None => {
            tracing::warn!(
                provider = %usage.provider,
                model = %usage.model,
                "no token price cached; returning zero"
            );
            return Ok(Decimal::ZERO);
        }
    };

    let input_rate: Decimal = input_per_mtok.parse().context("parse input token rate")?;
    let output_rate: Decimal = output_per_mtok.parse().context("parse output token rate")?;

    let input_m = Decimal::from(usage.input_tokens_per_month) / Decimal::from(1_000_000u64);
    let output_m = Decimal::from(usage.output_tokens_per_month) / Decimal::from(1_000_000u64);

    Ok(input_m * input_rate + output_m * output_rate)
}
