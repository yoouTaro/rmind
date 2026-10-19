//! What tokens cost, in USD per million, at Anthropic's API prices. Claude
//! Code caches prompts for an hour, and an hour-long cache write costs twice
//! the input price.
//!
//! On a subscription you don't pay these; they show what the work would have
//! cost on the API, which is still the best way to compare sessions.

/// (input, output, cache read, cache write) per million tokens.
pub fn per_million(model: &str) -> Option<(f64, f64, f64, f64)> {
    let m = model.split('[').next().unwrap_or(model);
    let (input, output, cache_read) = match m {
        _ if m.starts_with("claude-fable-5") || m.starts_with("claude-mythos-5") => (10.0, 50.0, 0.25),
        _ if m.starts_with("claude-opus-5-5") => (4.0, 20.0, 0.20),
        _ if m.starts_with("claude-opus-5") || m.starts_with("claude-opus-4") => (5.0, 25.0, 0.50),
        _ if m.starts_with("claude-sonnet-5") => (2.0, 10.0, 0.20),
        _ if m.starts_with("claude-sonnet-4") => (3.0, 15.0, 0.30),
        _ if m.starts_with("claude-haiku-5") => (0.10, 0.50, 0.01),
        _ if m.starts_with("claude-haiku-4") => (1.0, 5.0, 0.10),
        _ => return None,
    };
    Some((input, output, cache_read, input * 2.0))
}

/// USD for one response, or 0 for a model with unknown prices.
pub fn cost(model: &str, input: i64, output: i64, cache_read: i64, cache_write: i64) -> f64 {
    let Some((i, o, r, w)) = per_million(model) else { return 0.0 };
    (input as f64 * i + output as f64 * o + cache_read as f64 * r + cache_write as f64 * w) / 1e6
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices() {
        // 1M of each on Opus 5.5: 4 + 20 + 0.2 + 8.
        assert!((cost("claude-opus-5-5[1m]", 1_000_000, 1_000_000, 1_000_000, 1_000_000) - 32.2).abs() < 1e-9);
        assert!(per_million("claude-sonnet-5-5").is_some());
        assert_eq!(cost("gpt-9", 1, 1, 1, 1), 0.0);
    }
}
