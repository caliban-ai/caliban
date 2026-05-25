//! Per-image token-cost heuristic.
//!
//! Approximates Anthropic's documented `ceil(w * h / 750)` rule for
//! default-detail image inputs. The actual billed-tokens come from the
//! provider's `Usage` response; this heuristic is for diagnostic UI (the
//! `/usage` overlay's "est. tokens" column) and `OTel`
//! `caliban.token.usage{type=image}` tagging.

/// Approximate vision-input tokens for an image of (width, height).
///
/// The constant `750` matches Anthropic's published heuristic for the
/// default-detail image processing tier; `OpenAI` and Google's heuristics are
/// in the same ballpark.
#[must_use]
pub fn image_to_tokens(dims: (u32, u32)) -> u32 {
    let (w, h) = dims;
    let area = u64::from(w) * u64::from(h);
    // ceil(area / 750)
    let tokens = area.div_ceil(750);
    u32::try_from(tokens).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_image_is_zero_tokens() {
        assert_eq!(image_to_tokens((0, 0)), 0);
    }

    #[test]
    fn small_image_rounds_up() {
        // 100*100 = 10_000; 10_000 / 750 = 13.33 → 14
        assert_eq!(image_to_tokens((100, 100)), 14);
    }

    #[test]
    fn max_anthropic_image_about_3000_tokens() {
        // 1568 * 1568 = 2_458_624; /750 = 3278.16 → 3279
        let t = image_to_tokens((1568, 1568));
        assert!(
            (3000..=3300).contains(&t),
            "max-size image should be ~3000 tokens, got {t}",
        );
    }
}
