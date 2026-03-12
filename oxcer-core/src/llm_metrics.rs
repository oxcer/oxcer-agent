//! LLM token estimation and cost (Sprint 8 §4).
//!
//! Used by the LLM invoke path to compute tokens_in/out (estimate when no
//! provider tokenizer) and cost_usd from configured pricing per 1M tokens.

/// Estimate token count from character count (no tokenizer).
/// Common heuristic: ~4 chars per token for English/code.
pub fn estimate_tokens_from_chars(text: &str) -> u32 {
    ((text.len() / 4).max(1)) as u32
}

/// Provider name for telemetry: openai | gemini | anthropic | grok | local.
pub fn provider_for_model(model_id: &str) -> &'static str {
    if model_id.starts_with("gpt-") || model_id.starts_with("o1-") || model_id.starts_with("o3-") {
        "openai"
    } else if model_id.starts_with("gemini-") {
        "gemini"
    } else if model_id.starts_with("claude-") {
        "anthropic"
    } else if model_id.starts_with("grok-") {
        "grok"
    } else {
        "local"
    }
}

/// Price per 1M input tokens, price per 1M output tokens (USD).
/// Stub table; replace with config or real pricing.
fn price_per_million(provider: &str, model_id: &str) -> (f64, f64) {
    match provider {
        "openai" => {
            if model_id.starts_with("gpt-4o-mini") || model_id.starts_with("gpt-4.1-mini") {
                (0.15, 0.60)
            } else if model_id.starts_with("gpt-4o") || model_id.starts_with("gpt-4.1") {
                (2.50, 10.00)
            } else {
                (0.50, 1.50)
            }
        }
        "gemini" => {
            if model_id.contains("flash") {
                (0.075, 0.30)
            } else if model_id.contains("pro") {
                (1.25, 5.00)
            } else {
                (0.25, 0.50)
            }
        }
        "anthropic" => {
            if model_id.contains("sonnet") {
                (3.00, 15.00)
            } else if model_id.contains("haiku") {
                (0.25, 1.25)
            } else {
                (1.00, 5.00)
            }
        }
        "grok" => {
            if model_id.contains("fast") || model_id.contains("mini") {
                (0.10, 0.40)
            } else {
                (0.50, 2.00)
            }
        }
        _ => (0.0, 0.0),
    }
}

/// Compute cost in USD for given input/output token counts.
pub fn cost_usd(provider: &str, model_id: &str, tokens_in: u32, tokens_out: u32) -> f64 {
    let (in_per_m, out_per_m) = price_per_million(provider, model_id);
    let in_cost = (tokens_in as f64 / 1_000_000.0) * in_per_m;
    let out_cost = (tokens_out as f64 / 1_000_000.0) * out_per_m;
    in_cost + out_cost
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_from_chars_rounds_up() {
        assert_eq!(estimate_tokens_from_chars(""), 1);
        assert_eq!(estimate_tokens_from_chars("abcd"), 1);
        assert_eq!(estimate_tokens_from_chars("abcdefgh"), 2);
    }

    #[test]
    fn provider_for_model_prefixes() {
        assert_eq!(provider_for_model("gpt-4o-mini"), "openai");
        assert_eq!(provider_for_model("gemini-2.5-flash"), "gemini");
        assert_eq!(provider_for_model("claude-3.5-sonnet-latest"), "anthropic");
        assert_eq!(provider_for_model("grok-4.1-fast"), "grok");
        assert_eq!(provider_for_model("unknown"), "local");
    }

    #[test]
    fn cost_usd_non_negative() {
        let c = cost_usd("gemini", "gemini-2.5-flash", 1000, 500);
        assert!(c >= 0.0);
    }

    /// Given token counts and known pricing, cost is computed correctly.
    #[test]
    fn cost_usd_computed_correctly() {
        // gemini flash: 0.075 per 1M in, 0.30 per 1M out (from price_per_million)
        let c = cost_usd("gemini", "gemini-2.5-flash", 1_000_000, 1_000_000);
        let expected = 0.075 + 0.30;
        assert!(
            (c - expected).abs() < 1e-6,
            "expected {} got {}",
            expected,
            c
        );

        // 1000 in, 500 out -> 0.000075 + 0.00015 = 0.000225
        let c2 = cost_usd("gemini", "gemini-2.5-flash", 1000, 500);
        assert!((c2 - 0.000_225).abs() < 1e-9);

        // openai gpt-4o-mini: 0.15/1M in, 0.60/1M out
        let c3 = cost_usd("openai", "gpt-4o-mini", 1_000_000, 0);
        assert!((c3 - 0.15).abs() < 1e-6);
    }
}
