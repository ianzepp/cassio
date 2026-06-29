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
    let cost = estimate_cost(
        Some("opus-4.5"),
        1_000_000,
        500_000,
        2_000_000,
        200_000,
        None,
    )
    .unwrap();
    assert!((cost - 19.75).abs() < 0.001);
}

#[test]
fn test_estimate_cost_with_override() {
    let ovr = TokenPrice {
        input: 10.0,
        output: 20.0,
        cache_read: 1.0,
        cache_write: 2.0,
    };
    let cost = estimate_cost(
        Some("anything"),
        1_000_000,
        1_000_000,
        1_000_000,
        1_000_000,
        Some(ovr),
    )
    .unwrap();
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
