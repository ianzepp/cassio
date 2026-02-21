/// Token-to-cost estimation based on model name matching.
///
/// Provides a built-in price table for common AI coding models and a public
/// `estimate_cost` function used by the summary module. Prices are per million
/// tokens (input and output separately) and matched by substring against the
/// model name.
///
/// Users can override prices via config (`cassio set pricing.input <rate>` and
/// `cassio set pricing.output <rate>`) which bypass model matching entirely.

/// Per-million-token pricing for a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenPrice {
    /// Cost per million input tokens (USD).
    pub input: f64,
    /// Cost per million output tokens (USD).
    pub output: f64,
}

/// Built-in price table. Entries are checked in order; the first substring
/// match wins. More specific patterns come first so "haiku" doesn't match
/// before "opus".
const PRICE_TABLE: &[(&str, TokenPrice)] = &[
    // Anthropic
    ("opus", TokenPrice { input: 5.0, output: 25.0 }),
    ("sonnet", TokenPrice { input: 3.0, output: 15.0 }),
    ("haiku", TokenPrice { input: 1.0, output: 5.0 }),
    // OpenAI / Codex
    ("o3-pro", TokenPrice { input: 20.0, output: 80.0 }),
    ("o3-mini", TokenPrice { input: 1.10, output: 4.40 }),
    ("o3", TokenPrice { input: 2.0, output: 8.0 }),
    ("o4-mini", TokenPrice { input: 1.10, output: 4.40 }),
    ("gpt-4.1", TokenPrice { input: 2.0, output: 8.0 }),
    ("gpt-4.1-mini", TokenPrice { input: 0.40, output: 1.60 }),
    ("gpt-4.1-nano", TokenPrice { input: 0.10, output: 0.40 }),
    ("gpt-4o", TokenPrice { input: 2.50, output: 10.0 }),
    ("gpt-4o-mini", TokenPrice { input: 0.15, output: 0.60 }),
    ("codex-mini", TokenPrice { input: 1.50, output: 6.0 }),
];

/// Look up pricing for a model name (case-insensitive substring match).
pub fn lookup(model: &str) -> Option<TokenPrice> {
    let lower = model.to_lowercase();
    for (pattern, price) in PRICE_TABLE {
        if lower.contains(pattern) {
            return Some(*price);
        }
    }
    None
}

/// Estimate the USD cost for a given number of input and output tokens.
///
/// `price_override` allows config-driven pricing that bypasses model lookup.
/// Returns `None` if the model is unknown and no override is provided.
pub fn estimate_cost(
    model: Option<&str>,
    input_tokens: u64,
    output_tokens: u64,
    price_override: Option<TokenPrice>,
) -> Option<f64> {
    let price = price_override.or_else(|| model.and_then(lookup))?;
    let input_cost = input_tokens as f64 * price.input / 1_000_000.0;
    let output_cost = output_tokens as f64 * price.output / 1_000_000.0;
    Some(input_cost + output_cost)
}

/// Format a USD cost for display: "$1.23" or "<$0.01" for tiny amounts.
pub fn format_cost(cost: f64) -> String {
    if cost < 0.005 {
        "<$0.01".to_string()
    } else if cost < 1.0 {
        format!("${:.2}", cost)
    } else if cost < 100.0 {
        format!("${:.2}", cost)
    } else {
        format!("${:.0}", cost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_claude_opus() {
        let p = lookup("claude-opus-4-5-20251101").unwrap();
        assert_eq!(p.input, 5.0);
        assert_eq!(p.output, 25.0);
    }

    #[test]
    fn test_lookup_claude_sonnet() {
        let p = lookup("claude-sonnet-4-5-20250929").unwrap();
        assert_eq!(p.input, 3.0);
        assert_eq!(p.output, 15.0);
    }

    #[test]
    fn test_lookup_claude_haiku() {
        let p = lookup("claude-haiku-4-5-20251001").unwrap();
        assert_eq!(p.input, 1.0);
        assert_eq!(p.output, 5.0);
    }

    #[test]
    fn test_lookup_shortened_model_name() {
        let p = lookup("opus-4.5").unwrap();
        assert_eq!(p.input, 5.0);
    }

    #[test]
    fn test_lookup_codex_model() {
        let p = lookup("codex-mini").unwrap();
        assert_eq!(p.input, 1.50);
        assert_eq!(p.output, 6.0);
    }

    #[test]
    fn test_lookup_o3_pro() {
        let p = lookup("o3-pro").unwrap();
        assert_eq!(p.input, 20.0);
    }

    #[test]
    fn test_lookup_unknown() {
        assert!(lookup("some-random-model").is_none());
    }

    #[test]
    fn test_lookup_case_insensitive() {
        assert!(lookup("Claude-OPUS-4-5").is_some());
    }

    #[test]
    fn test_estimate_cost_basic() {
        // 1M input tokens at $5/MTok + 500K output at $25/MTok = $5 + $12.50
        let cost = estimate_cost(Some("opus-4.5"), 1_000_000, 500_000, None).unwrap();
        assert!((cost - 17.50).abs() < 0.001);
    }

    #[test]
    fn test_estimate_cost_with_override() {
        let ovr = TokenPrice { input: 10.0, output: 20.0 };
        let cost = estimate_cost(Some("anything"), 1_000_000, 1_000_000, Some(ovr)).unwrap();
        assert!((cost - 30.0).abs() < 0.001);
    }

    #[test]
    fn test_estimate_cost_unknown_no_override() {
        assert!(estimate_cost(Some("unknown-model"), 1000, 1000, None).is_none());
    }

    #[test]
    fn test_estimate_cost_no_model_no_override() {
        assert!(estimate_cost(None, 1000, 1000, None).is_none());
    }

    #[test]
    fn test_format_cost_small() {
        assert_eq!(format_cost(0.001), "<$0.01");
    }

    #[test]
    fn test_format_cost_cents() {
        assert_eq!(format_cost(0.50), "$0.50");
    }

    #[test]
    fn test_format_cost_dollars() {
        assert_eq!(format_cost(12.345), "$12.35");
    }

    #[test]
    fn test_format_cost_large() {
        assert_eq!(format_cost(150.0), "$150");
    }
}
