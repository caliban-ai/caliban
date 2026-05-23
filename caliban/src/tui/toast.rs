//! Ephemeral one-row notification rendered above the input area.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub(crate) enum ToastLevel {
    Error,
    Warn,
    Info,
}

#[derive(Debug)]
pub(crate) struct Toast {
    pub(crate) level: ToastLevel,
    pub(crate) text: String,
    shown_at: Instant,
    ttl: Duration,
}

impl Toast {
    #[allow(dead_code, reason = "wired in T8 attachment submit pipeline")]
    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self::new(ToastLevel::Error, text)
    }

    #[allow(dead_code, reason = "warn/info will be used by later slices")]
    pub(crate) fn warn(text: impl Into<String>) -> Self {
        Self::new(ToastLevel::Warn, text)
    }

    #[allow(dead_code, reason = "warn/info will be used by later slices")]
    pub(crate) fn info(text: impl Into<String>) -> Self {
        Self::new(ToastLevel::Info, text)
    }

    fn new(level: ToastLevel, text: impl Into<String>) -> Self {
        Self {
            level,
            text: text.into(),
            shown_at: Instant::now(),
            ttl: Duration::from_secs(5),
        }
    }

    pub(crate) fn is_expired(&self) -> bool {
        self.shown_at.elapsed() >= self.ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_toast_not_expired() {
        let t = Toast::error("boom");
        assert!(!t.is_expired());
    }

    #[test]
    fn level_preserved() {
        assert!(matches!(Toast::error("x").level, ToastLevel::Error));
        assert!(matches!(Toast::warn("x").level, ToastLevel::Warn));
        assert!(matches!(Toast::info("x").level, ToastLevel::Info));
    }
}
