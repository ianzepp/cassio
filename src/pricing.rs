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
    ("opus",   TokenPrice { input: 5.0,  output: 25.0, cache_read: 0.50,  cache_write: 6.25  }),
    ("sonnet", TokenPrice { input: 3.0,  output: 15.0, cache_read: 0.30,  cache_write: 3.75  }),
    ("haiku",  TokenPrice { input: 1.0,  output: 5.0,  cache_read: 0.10,  cache_write: 1.25  }),
    // OpenAI / Codex — no published cache pricing; zero out cache fields
    // More-specific patterns first to prevent prefix collisions (e.g. "gpt-5.1-codex-mini"
    // before "gpt-5.1-codex", "gpt-5.3-codex" before "gpt-5.3", etc.)
    ("o3-pro",           TokenPrice { input: 20.0,  output: 80.0,  cache_read: 0.0, cache_write: 0.0 }),
    ("o3-mini",          TokenPrice { input: 1.10,  output: 4.40,  cache_read: 0.0, cache_write: 0.0 }),
    ("o3",               TokenPrice { input: 2.0,   output: 8.0,   cache_read: 0.0, cache_write: 0.0 }),
    ("o4-mini",          TokenPrice { input: 1.10,  output: 4.40,  cache_read: 0.0, cache_write: 0.0 }),
    // GPT-5.3
    ("gpt-5.3-codex",    TokenPrice { input: 1.75,  output: 14.0,  cache_read: 0.0, cache_write: 0.0 }),
    ("gpt-5.3",          TokenPrice { input: 1.75,  output: 14.0,  cache_read: 0.0, cache_write: 0.0 }),
    // GPT-5.2
    ("gpt-5.2-codex",    TokenPrice { input: 1.75,  output: 14.0,  cache_read: 0.0, cache_write: 0.0 }),
    ("gpt-5.2",          TokenPrice { input: 1.75,  output: 14.0,  cache_read: 0.0, cache_write: 0.0 }),
    // GPT-5.1
    ("gpt-5.1-codex-mini", TokenPrice { input: 0.25, output: 2.0,  cache_read: 0.0, cache_write: 0.0 }),
    ("gpt-5.1-codex",    TokenPrice { input: 1.25,  output: 10.0,  cache_read: 0.0, cache_write: 0.0 }),
    ("gpt-5.1",          TokenPrice { input: 1.25,  output: 10.0,  cache_read: 0.0, cache_write: 0.0 }),
    // GPT-5 — nano/mini must come before bare "gpt-5" to avoid false match
    ("gpt-5-nano",       TokenPrice { input: 0.05,  output: 0.40,  cache_read: 0.005, cache_write: 0.0 }),
    ("gpt-5-codex",      TokenPrice { input: 1.25,  output: 10.0,  cache_read: 0.0,   cache_write: 0.0 }),
    ("gpt-5",            TokenPrice { input: 1.25,  output: 10.0,  cache_read: 0.0,   cache_write: 0.0 }),
    // GPT-4.x
    ("gpt-4.1-mini",     TokenPrice { input: 0.40,  output: 1.60,  cache_read: 0.0, cache_write: 0.0 }),
    ("gpt-4.1-nano",     TokenPrice { input: 0.10,  output: 0.40,  cache_read: 0.0, cache_write: 0.0 }),
    ("gpt-4.1",          TokenPrice { input: 2.0,   output: 8.0,   cache_read: 0.0, cache_write: 0.0 }),
    ("gpt-4o-mini",      TokenPrice { input: 0.15,  output: 0.60,  cache_read: 0.0, cache_write: 0.0 }),
    ("gpt-4o",           TokenPrice { input: 2.50,  output: 10.0,  cache_read: 0.0, cache_write: 0.0 }),
    ("codex-mini",       TokenPrice { input: 1.50,  output: 6.0,   cache_read: 0.0, cache_write: 0.0 }),
    // xAI Grok — grok-code-fast before grok-code, grok-4.1-fast before grok-4-fast before grok-4
    ("grok-code-fast",   TokenPrice { input: 0.20,  output: 1.50,  cache_read: 0.02, cache_write: 0.0 }),
    ("grok-code",        TokenPrice { input: 0.20,  output: 1.50,  cache_read: 0.02, cache_write: 0.0 }),
    ("grok-4.1-fast",    TokenPrice { input: 0.20,  output: 0.50,  cache_read: 0.05, cache_write: 0.0 }),
    ("grok-4-fast",      TokenPrice { input: 0.20,  output: 0.50,  cache_read: 0.05, cache_write: 0.0 }),
    ("grok-4",           TokenPrice { input: 3.0,   output: 15.0,  cache_read: 0.0,  cache_write: 0.0 }),
    // Moonshot Kimi — kimi-k2-thinking before kimi-k2
    ("kimi-k2-thinking", TokenPrice { input: 0.60,  output: 2.50,  cache_read: 0.15, cache_write: 0.0 }),
    ("kimi-k2",          TokenPrice { input: 0.60,  output: 2.50,  cache_read: 0.15, cache_write: 0.0 }),
    // Zhipu GLM — most specific first
    ("glm-4.7-flash",    TokenPrice { input: 0.0,   output: 0.0,   cache_read: 0.0,  cache_write: 0.0 }),
    ("glm-4.7-free",     TokenPrice { input: 0.0,   output: 0.0,   cache_read: 0.0,  cache_write: 0.0 }),
    ("glm-4.7",          TokenPrice { input: 0.60,  output: 2.20,  cache_read: 0.0,  cache_write: 0.0 }),
    ("glm-4.6",          TokenPrice { input: 0.30,  output: 0.90,  cache_read: 0.0,  cache_write: 0.0 }),
    ("glm-5",            TokenPrice { input: 1.0,   output: 3.20,  cache_read: 0.0,  cache_write: 0.0 }),
    ("glm-4",            TokenPrice { input: 0.60,  output: 2.20,  cache_read: 0.0,  cache_write: 0.0 }),
    // Google Gemini 3 — flash before pro
    ("gemini-3-flash",   TokenPrice { input: 0.50,  output: 3.0,   cache_read: 0.0,  cache_write: 0.0 }),
    ("gemini-3-pro",     TokenPrice { input: 2.0,   output: 12.0,  cache_read: 0.0,  cache_write: 0.0 }),
    ("gemini-2.5-pro",   TokenPrice { input: 1.25,  output: 10.0,  cache_read: 0.0,  cache_write: 0.0 }),
    // Qwen
    ("qwen3-coder",      TokenPrice { input: 0.22,  output: 1.0,   cache_read: 0.0,  cache_write: 0.0 }),
    // DeepSeek — most specific first
    ("deepseek-chat-v3.1", TokenPrice { input: 0.15, output: 0.75, cache_read: 0.0,  cache_write: 0.0 }),
    ("deepseek-v3.2",    TokenPrice { input: 0.28,  output: 0.42,  cache_read: 0.028, cache_write: 0.0 }),
    ("deepseek-v3",      TokenPrice { input: 0.27,  output: 1.10,  cache_read: 0.0,  cache_write: 0.0 }),
    ("deepseek-r1",      TokenPrice { input: 0.55,  output: 2.19,  cache_read: 0.0,  cache_write: 0.0 }),
    // MiniMax
    ("minimax-m2.5",     TokenPrice { input: 0.30,  output: 1.20,  cache_read: 0.0,  cache_write: 0.0 }),
    ("minimax-m2",       TokenPrice { input: 0.30,  output: 1.20,  cache_read: 0.0,  cache_write: 0.0 }),
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
    fn test_lookup_gpt5_codex_variants() {
        // gpt-5.1-codex-mini must not match gpt-5.1-codex or gpt-5.1
        let mini = lookup("gpt-5.1-codex-mini").unwrap();
        assert_eq!(mini.input, 0.25);
        assert_eq!(mini.output, 2.0);

        let codex = lookup("gpt-5.1-codex").unwrap();
        assert_eq!(codex.input, 1.25);

        let base = lookup("gpt-5.1").unwrap();
        assert_eq!(base.input, 1.25);
    }

    #[test]
    fn test_lookup_gpt52_codex() {
        let p = lookup("gpt-5.2-codex").unwrap();
        assert_eq!(p.input, 1.75);
        assert_eq!(p.output, 14.0);
    }

    #[test]
    fn test_lookup_gpt52_base() {
        // gpt-5.2 (no codex suffix) should still resolve
        let p = lookup("gpt-5.2").unwrap();
        assert_eq!(p.input, 1.75);
        assert_eq!(p.output, 14.0);
    }

    #[test]
    fn test_lookup_gpt53_codex() {
        let p = lookup("gpt-5.3-codex").unwrap();
        assert_eq!(p.input, 1.75);
        assert_eq!(p.output, 14.0);
    }

    #[test]
    fn test_lookup_gpt5_codex() {
        let p = lookup("gpt-5-codex").unwrap();
        assert_eq!(p.input, 1.25);
        assert_eq!(p.output, 10.0);
    }

    #[test]
    fn test_lookup_gpt5_nano_not_matched_by_gpt5() {
        // gpt-5-nano must resolve to nano pricing, not the generic gpt-5 price
        let p = lookup("gpt-5-nano").unwrap();
        assert_eq!(p.input, 0.05);
        assert_eq!(p.output, 0.40);
    }

    #[test]
    fn test_lookup_grok_code_fast() {
        let p = lookup("x-ai/grok-code-fast-1").unwrap();
        assert_eq!(p.input, 0.20);
        assert_eq!(p.output, 1.50);
    }

    #[test]
    fn test_lookup_grok_4_1_fast() {
        let p = lookup("x-ai/grok-4.1-fast").unwrap();
        assert_eq!(p.input, 0.20);
        assert_eq!(p.output, 0.50);
    }

    #[test]
    fn test_lookup_kimi_k2_thinking_not_matched_by_kimi_k2() {
        // kimi-k2-thinking must resolve before kimi-k2 (same price here, but ordering verified)
        let thinking = lookup("moonshotai/kimi-k2-thinking").unwrap();
        let base = lookup("kimi-k2").unwrap();
        assert_eq!(thinking.input, base.input);
        let p = lookup("kimi-k2.5:cloud").unwrap();
        assert_eq!(p.input, 0.60);
    }

    #[test]
    fn test_lookup_glm_variants() {
        // flash/free are free
        assert_eq!(lookup("glm-4.7-flash").unwrap().input, 0.0);
        assert_eq!(lookup("glm-4.7-free").unwrap().input, 0.0);
        // paid glm-4.7 must not match flash/free pattern
        let p = lookup("glm-4.7").unwrap();
        assert_eq!(p.input, 0.60);
        assert_eq!(lookup("glm-5").unwrap().input, 1.0);
        // z-ai/glm-4.7 should hit glm-4.7
        assert_eq!(lookup("z-ai/glm-4.7").unwrap().input, 0.60);
    }

    #[test]
    fn test_lookup_gemini_3() {
        let flash = lookup("gemini-3-flash").unwrap();
        assert_eq!(flash.input, 0.50);
        assert_eq!(flash.output, 3.0);
        let pro = lookup("gemini-3-pro").unwrap();
        assert_eq!(pro.input, 2.0);
        assert_eq!(pro.output, 12.0);
    }

    #[test]
    fn test_lookup_deepseek_variants() {
        let v31 = lookup("deepseek/deepseek-chat-v3.1").unwrap();
        assert_eq!(v31.input, 0.15);
        let v32 = lookup("deepseek/deepseek-v3.2").unwrap();
        assert_eq!(v32.input, 0.28);
    }

    #[test]
    fn test_lookup_qwen3_coder() {
        let p = lookup("qwen/qwen3-coder-flash").unwrap();
        assert_eq!(p.input, 0.22);
        assert_eq!(p.output, 1.0);
    }

    #[test]
    fn test_lookup_minimax() {
        let p = lookup("minimax/minimax-m2.5").unwrap();
        assert_eq!(p.input, 0.30);
        assert_eq!(p.output, 1.20);
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
        let cost = estimate_cost(Some("opus-4.5"), 1_000_000, 500_000, 0, 0, None).unwrap();
        assert!((cost - 17.50).abs() < 0.001);
    }

    #[test]
    fn test_estimate_cost_with_cache() {
        // opus: input=$5, output=$25, cache_read=$0.50, cache_write=$6.25 per MTok
        // 1M input=$5, 500K output=$12.50, 2M cache_read=$1.00, 200K cache_write=$1.25 → $19.75
        let cost = estimate_cost(Some("opus-4.5"), 1_000_000, 500_000, 2_000_000, 200_000, None).unwrap();
        assert!((cost - 19.75).abs() < 0.001);
    }

    #[test]
    fn test_estimate_cost_with_override() {
        let ovr = TokenPrice { input: 10.0, output: 20.0, cache_read: 1.0, cache_write: 2.0 };
        let cost = estimate_cost(Some("anything"), 1_000_000, 1_000_000, 1_000_000, 1_000_000, Some(ovr)).unwrap();
        assert!((cost - 33.0).abs() < 0.001);
    }

    #[test]
    fn test_estimate_cost_unknown_no_override() {
        assert!(estimate_cost(Some("unknown-model"), 1000, 1000, 0, 0, None).is_none());
    }

    #[test]
    fn test_estimate_cost_no_model_no_override() {
        assert!(estimate_cost(None, 1000, 1000, 0, 0, None).is_none());
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
