//! Token-to-cost estimation based on model name matching.
//!
//! Provides a built-in price table for common AI coding models and a public
//! `estimate_cost` function used by the summary module. Prices are per million
//! tokens (input and output separately) and matched by substring against the
//! model name.
//!
//! Users can override prices via config (`cassio set pricing.input <rate>` and
//! `cassio set pricing.output <rate>`) which bypass model matching entirely.

/// Per-million-token pricing for a model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenPrice {
    /// Cost per million input tokens (USD).
    pub input: f64,
    /// Cost per million output tokens (USD).
    pub output: f64,
    /// Cost per million cache-read tokens (USD). Typically ~10% of input rate.
    pub cache_read: f64,
    /// Cost per million cache-creation tokens (USD). Typically ~125% of input rate.
    pub cache_write: f64,
}

/// Built-in price table. Entries are checked in order; the first substring
/// match wins. More specific patterns come first so "haiku" doesn't match
/// before "opus".
const PRICE_TABLE: &[(&str, TokenPrice)] = &[
    // Anthropic — cache_read = 10% of input, cache_write = 125% of input
    (
        "opus",
        TokenPrice {
            input: 5.0,
            output: 25.0,
            cache_read: 0.50,
            cache_write: 6.25,
        },
    ),
    (
        "sonnet",
        TokenPrice {
            input: 3.0,
            output: 15.0,
            cache_read: 0.30,
            cache_write: 3.75,
        },
    ),
    (
        "haiku",
        TokenPrice {
            input: 1.0,
            output: 5.0,
            cache_read: 0.10,
            cache_write: 1.25,
        },
    ),
    // OpenAI / Codex — no published cache pricing; zero out cache fields
    // More-specific patterns first to prevent prefix collisions (e.g. "gpt-5.1-codex-mini"
    // before "gpt-5.1-codex", "gpt-5.3-codex" before "gpt-5.3", etc.)
    (
        "o3-pro",
        TokenPrice {
            input: 20.0,
            output: 80.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "o3-mini",
        TokenPrice {
            input: 1.10,
            output: 4.40,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "o3",
        TokenPrice {
            input: 2.0,
            output: 8.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "o4-mini",
        TokenPrice {
            input: 1.10,
            output: 4.40,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // GPT-5.3
    (
        "gpt-5.3-codex",
        TokenPrice {
            input: 1.75,
            output: 14.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.3",
        TokenPrice {
            input: 1.75,
            output: 14.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // GPT-5.2
    (
        "gpt-5.2-codex",
        TokenPrice {
            input: 1.75,
            output: 14.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.2",
        TokenPrice {
            input: 1.75,
            output: 14.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // GPT-5.1
    (
        "gpt-5.1-codex-mini",
        TokenPrice {
            input: 0.25,
            output: 2.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.1-codex",
        TokenPrice {
            input: 1.25,
            output: 10.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5.1",
        TokenPrice {
            input: 1.25,
            output: 10.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // GPT-5 — nano/mini must come before bare "gpt-5" to avoid false match
    (
        "gpt-5-nano",
        TokenPrice {
            input: 0.05,
            output: 0.40,
            cache_read: 0.005,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5-codex",
        TokenPrice {
            input: 1.25,
            output: 10.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-5",
        TokenPrice {
            input: 1.25,
            output: 10.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // GPT-4.x
    (
        "gpt-4.1-mini",
        TokenPrice {
            input: 0.40,
            output: 1.60,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-4.1-nano",
        TokenPrice {
            input: 0.10,
            output: 0.40,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-4.1",
        TokenPrice {
            input: 2.0,
            output: 8.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-4o-mini",
        TokenPrice {
            input: 0.15,
            output: 0.60,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-4o",
        TokenPrice {
            input: 2.50,
            output: 10.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "codex-mini",
        TokenPrice {
            input: 1.50,
            output: 6.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // xAI Grok — grok-code-fast before grok-code, grok-4.1-fast before grok-4-fast before grok-4
    (
        "grok-code-fast",
        TokenPrice {
            input: 0.20,
            output: 1.50,
            cache_read: 0.02,
            cache_write: 0.0,
        },
    ),
    (
        "grok-code",
        TokenPrice {
            input: 0.20,
            output: 1.50,
            cache_read: 0.02,
            cache_write: 0.0,
        },
    ),
    (
        "grok-4.1-fast",
        TokenPrice {
            input: 0.20,
            output: 0.50,
            cache_read: 0.05,
            cache_write: 0.0,
        },
    ),
    (
        "grok-4-fast",
        TokenPrice {
            input: 0.20,
            output: 0.50,
            cache_read: 0.05,
            cache_write: 0.0,
        },
    ),
    (
        "grok-4",
        TokenPrice {
            input: 3.0,
            output: 15.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // Moonshot Kimi — kimi-k2-thinking before kimi-k2
    (
        "kimi-k2-thinking",
        TokenPrice {
            input: 0.60,
            output: 2.50,
            cache_read: 0.15,
            cache_write: 0.0,
        },
    ),
    (
        "kimi-k2",
        TokenPrice {
            input: 0.60,
            output: 2.50,
            cache_read: 0.15,
            cache_write: 0.0,
        },
    ),
    // Zhipu GLM — most specific first
    (
        "glm-4.7-flash",
        TokenPrice {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "glm-4.7-free",
        TokenPrice {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "glm-4.7",
        TokenPrice {
            input: 0.60,
            output: 2.20,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "glm-4.6",
        TokenPrice {
            input: 0.30,
            output: 0.90,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "glm-5",
        TokenPrice {
            input: 1.0,
            output: 3.20,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "glm-4",
        TokenPrice {
            input: 0.60,
            output: 2.20,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // Google Gemini 3 — flash before pro
    (
        "gemini-3-flash",
        TokenPrice {
            input: 0.50,
            output: 3.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-3-pro",
        TokenPrice {
            input: 2.0,
            output: 12.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-2.5-pro",
        TokenPrice {
            input: 1.25,
            output: 10.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // Qwen
    (
        "qwen3-coder",
        TokenPrice {
            input: 0.22,
            output: 1.0,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // DeepSeek — most specific first
    (
        "deepseek-chat-v3.1",
        TokenPrice {
            input: 0.15,
            output: 0.75,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "deepseek-v3.2",
        TokenPrice {
            input: 0.28,
            output: 0.42,
            cache_read: 0.028,
            cache_write: 0.0,
        },
    ),
    (
        "deepseek-v3",
        TokenPrice {
            input: 0.27,
            output: 1.10,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "deepseek-r1",
        TokenPrice {
            input: 0.55,
            output: 2.19,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    // MiniMax
    (
        "minimax-m2.5",
        TokenPrice {
            input: 0.30,
            output: 1.20,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
    (
        "minimax-m2",
        TokenPrice {
            input: 0.30,
            output: 1.20,
            cache_read: 0.0,
            cache_write: 0.0,
        },
    ),
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

/// Estimate the USD cost for a given number of tokens including cache.
///
/// `price_override` allows config-driven pricing that bypasses model lookup.
/// Returns `None` if the model is unknown and no override is provided.
pub fn estimate_cost(
    model: Option<&str>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    price_override: Option<TokenPrice>,
) -> Option<f64> {
    let price = price_override.or_else(|| model.and_then(lookup))?;
    let input_cost = input_tokens as f64 * price.input / 1_000_000.0;
    let output_cost = output_tokens as f64 * price.output / 1_000_000.0;
    let cache_read_cost = cache_read_tokens as f64 * price.cache_read / 1_000_000.0;
    let cache_write_cost = cache_write_tokens as f64 * price.cache_write / 1_000_000.0;
    Some(input_cost + output_cost + cache_read_cost + cache_write_cost)
}

/// Format a USD cost for display: "$1.23" or "<$0.01" for tiny amounts.
pub fn format_cost(cost: f64) -> String {
    if cost < 0.005 {
        "<$0.01".to_string()
    } else if cost < 100.0 {
        format!("${:.2}", cost)
    } else {
        format!("${:.0}", cost)
    }
}

#[cfg(test)]
#[path = "pricing_test.rs"]
mod tests;
