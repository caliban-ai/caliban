//! Terminal graphics protocol detection + image rendering.
//!
//! The detection cascade is intentionally light-weight:
//!
//! 1. `CALIBAN_GRAPHICS` env override (`kitty` / `iterm` / `sixel` / `none`).
//! 2. `$TERM_PROGRAM` heuristic (iTerm2.app).
//! 3. `$TERM` heuristic (`xterm-kitty`, `*-sixel`).
//! 4. Fallback: text placeholder.
//!
//! Live escape-sequence capability probes (`\x1b_Gi=…\x1b\\`) are out of
//! scope for v1 — the env-driven path is deterministic, tests cleanly, and
//! the user-visible escape valve (`CALIBAN_GRAPHICS=none`) covers the
//! awkward-terminal case.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

/// Terminal graphics protocols recognized by caliban.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsProtocol {
    /// Kitty graphics protocol — `\x1b_Ga=T,f=100,...\x1b\\`.
    Kitty,
    /// iTerm2 inline-image protocol — `\x1b]1337;File=…\x07`.
    ITerm2,
    /// DEC sixel — `\x1bPq…\x1b\\`.
    Sixel,
    /// No supported protocol; render text placeholder.
    None,
}

/// Detect the graphics protocol from a set of env vars.
///
/// `graphics_override` is `$CALIBAN_GRAPHICS` (or `None` if unset).
/// `term_program` is `$TERM_PROGRAM`. `term` is `$TERM`.
#[must_use]
pub fn detect_graphics_protocol(
    graphics_override: Option<&str>,
    term_program: Option<&str>,
    term: Option<&str>,
) -> GraphicsProtocol {
    if let Some(o) = graphics_override {
        return match o.to_ascii_lowercase().as_str() {
            "kitty" => GraphicsProtocol::Kitty,
            "iterm" | "iterm2" => GraphicsProtocol::ITerm2,
            "sixel" => GraphicsProtocol::Sixel,
            _ => GraphicsProtocol::None,
        };
    }
    if let Some(tp) = term_program
        && (tp.eq_ignore_ascii_case("iTerm.app") || tp.eq_ignore_ascii_case("WezTerm"))
    {
        // WezTerm groks both iTerm2 and kitty escapes; pick iTerm2's
        // shape because it's denser on the wire.
        return GraphicsProtocol::ITerm2;
    }
    if let Some(t) = term {
        let lower = t.to_ascii_lowercase();
        if lower.contains("kitty") {
            return GraphicsProtocol::Kitty;
        }
        if lower.contains("sixel") {
            return GraphicsProtocol::Sixel;
        }
    }
    GraphicsProtocol::None
}

/// Render an image for the given protocol. PNG bytes input.
///
/// For non-graphics terminals this returns `None`; the caller is expected
/// to fall back to [`crate::placeholder::text_placeholder`].
#[must_use]
pub fn render_for_protocol(protocol: GraphicsProtocol, png_bytes: &[u8]) -> Option<String> {
    match protocol {
        GraphicsProtocol::Kitty => Some(render_kitty(png_bytes)),
        GraphicsProtocol::ITerm2 => Some(render_iterm2(png_bytes)),
        GraphicsProtocol::Sixel => Some(render_sixel(png_bytes)),
        GraphicsProtocol::None => None,
    }
}

fn render_kitty(png_bytes: &[u8]) -> String {
    let data = BASE64.encode(png_bytes);
    // a=T = transmit + display, f=100 = PNG, m=0 = no more chunks.
    format!("\x1b_Ga=T,f=100,m=0;{data}\x1b\\")
}

fn render_iterm2(png_bytes: &[u8]) -> String {
    let data = BASE64.encode(png_bytes);
    let size = png_bytes.len();
    format!("\x1b]1337;File=inline=1;size={size}:{data}\x07")
}

fn render_sixel(png_bytes: &[u8]) -> String {
    // We don't carry a sixel encoder; emit a sixel introducer + an empty
    // body. Sixel-capable terminals tolerate this; the caller can layer a
    // real encoder later (the `sixel` crate is the natural fit, but pulls
    // a C dep and we opt out of it in v1).
    let _ = png_bytes;
    "\x1bPq\x1b\\".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins() {
        assert_eq!(
            detect_graphics_protocol(
                Some("kitty"),
                Some("Apple_Terminal"),
                Some("xterm-256color")
            ),
            GraphicsProtocol::Kitty,
        );
        assert_eq!(
            detect_graphics_protocol(Some("none"), Some("iTerm.app"), Some("xterm-kitty")),
            GraphicsProtocol::None,
        );
    }

    #[test]
    fn iterm2_term_program() {
        assert_eq!(
            detect_graphics_protocol(None, Some("iTerm.app"), None),
            GraphicsProtocol::ITerm2,
        );
    }

    #[test]
    fn kitty_term_env() {
        assert_eq!(
            detect_graphics_protocol(None, None, Some("xterm-kitty")),
            GraphicsProtocol::Kitty,
        );
    }

    #[test]
    fn sixel_term_env() {
        assert_eq!(
            detect_graphics_protocol(None, None, Some("xterm-sixel")),
            GraphicsProtocol::Sixel,
        );
    }

    #[test]
    fn unknown_terminal_falls_back_to_none() {
        assert_eq!(
            detect_graphics_protocol(None, Some("Apple_Terminal"), Some("xterm-256color")),
            GraphicsProtocol::None,
        );
    }

    #[test]
    fn kitty_renderer_emits_apc_envelope() {
        let bytes = b"PNG";
        let out = render_for_protocol(GraphicsProtocol::Kitty, bytes).expect("kitty");
        assert!(out.starts_with("\x1b_G"));
        assert!(out.ends_with("\x1b\\"));
        assert!(out.contains("a=T"));
        assert!(out.contains("f=100"));
    }

    #[test]
    fn iterm2_renderer_emits_osc_1337_envelope() {
        let bytes = b"PNG";
        let out = render_for_protocol(GraphicsProtocol::ITerm2, bytes).expect("iterm2");
        assert!(out.starts_with("\x1b]1337;"));
        assert!(out.ends_with('\x07'));
    }

    #[test]
    fn sixel_renderer_emits_dcs_envelope() {
        let out = render_for_protocol(GraphicsProtocol::Sixel, b"x").expect("sixel");
        assert!(out.starts_with("\x1bPq"));
        assert!(out.ends_with("\x1b\\"));
    }

    #[test]
    fn none_protocol_returns_no_render() {
        assert!(render_for_protocol(GraphicsProtocol::None, b"x").is_none());
    }
}
