//! macOS Seatbelt (`.sb` profile) generation.
//!
//! The dialect is TinyScheme-flavored Lisp consumed by `sandbox-exec(1)`.
//! It is undocumented but stable; we mirror Chrome's renderer-sandbox
//! style and Claude Code's profile.

use std::fmt::Write as _;

use crate::config::Policy;

/// Render a complete Seatbelt `.sb` profile from `policy`.
#[must_use]
pub fn render_profile(policy: &Policy) -> String {
    let mut out = String::with_capacity(1024);
    let _ = writeln!(out, ";; caliban-sandbox Seatbelt profile (generated)");
    let _ = writeln!(out, "(version 1)");
    let _ = writeln!(out, "(deny default)");
    let _ = writeln!(out);

    let _ = writeln!(out, ";; Basic process operations the child needs.");
    let _ = writeln!(out, "(allow process-fork)");
    let _ = writeln!(out, "(allow process-exec)");
    let _ = writeln!(out, "(allow signal (target self))");
    let _ = writeln!(out, "(allow sysctl-read)");
    let _ = writeln!(out, "(allow file-read-metadata)");
    let _ = writeln!(out);

    // -- Reads ---------------------------------------------------------
    if !policy.filesystem.allow_read.is_empty() {
        let _ = writeln!(out, ";; Reads (allow_read).");
        let _ = writeln!(out, "(allow file-read*");
        for p in &policy.filesystem.allow_read {
            let _ = writeln!(out, "  (subpath {})", quote_path(&p.display().to_string()));
        }
        let _ = writeln!(out, ")");
    }
    if !policy.filesystem.deny_read.is_empty() {
        let _ = writeln!(out, ";; Read denies (deny_read).");
        let _ = writeln!(out, "(deny file-read*");
        for p in &policy.filesystem.deny_read {
            let _ = writeln!(out, "  (subpath {})", quote_path(&p.display().to_string()));
        }
        let _ = writeln!(out, ")");
    }

    // -- Writes --------------------------------------------------------
    if !policy.filesystem.allow_write.is_empty() {
        let _ = writeln!(out, ";; Writes (allow_write).");
        let _ = writeln!(out, "(allow file-write*");
        for p in &policy.filesystem.allow_write {
            let _ = writeln!(out, "  (subpath {})", quote_path(&p.display().to_string()));
        }
        let _ = writeln!(out, ")");
    }
    if !policy.filesystem.deny_write.is_empty() {
        let _ = writeln!(out, ";; Write denies (deny_write).");
        let _ = writeln!(out, "(deny file-write*");
        for p in &policy.filesystem.deny_write {
            let _ = writeln!(out, "  (subpath {})", quote_path(&p.display().to_string()));
        }
        let _ = writeln!(out, ")");
    }

    // -- Mach lookups (macOS-only) -------------------------------------
    if !policy.network.allow_mach_lookup.is_empty() {
        let _ = writeln!(out, ";; Mach service lookups.");
        for svc in &policy.network.allow_mach_lookup {
            let _ = writeln!(out, "(allow mach-lookup (global-name {}))", quote_path(svc));
        }
    }

    // -- Network -------------------------------------------------------
    let net = &policy.network;
    if net.http_proxy_port != 0 {
        // Lock egress to the operator-run HTTP proxy only.
        let _ = writeln!(
            out,
            ";; Network: HTTP proxy at 127.0.0.1:{}.",
            net.http_proxy_port
        );
        let _ = writeln!(
            out,
            "(allow network-outbound (remote tcp \"127.0.0.1:{}\"))",
            net.http_proxy_port
        );
    } else if net.socks_proxy_port != 0 {
        let _ = writeln!(
            out,
            ";; Network: SOCKS proxy at 127.0.0.1:{}.",
            net.socks_proxy_port
        );
        let _ = writeln!(
            out,
            "(allow network-outbound (remote tcp \"127.0.0.1:{}\"))",
            net.socks_proxy_port
        );
    } else if !net.allowed_domains.is_empty() {
        let _ = writeln!(out, ";; Network: allowed_domains (TCP/443).");
        let _ = writeln!(out, "(allow network-outbound");
        for d in &net.allowed_domains {
            let _ = writeln!(out, "  (remote tcp {})", quote_path(&format!("{d}:443")));
        }
        let _ = writeln!(out, ")");
    }
    // else: deny default already blocks egress.

    if net.allow_local_binding {
        let _ = writeln!(out, ";; Allow local bind.");
        let _ = writeln!(out, "(allow network-bind (local ip \"*:0\"))");
    }
    if net.allow_unix_sockets {
        let _ = writeln!(out, ";; Allow Unix-socket networking.");
        let _ = writeln!(out, "(allow network-outbound (remote unix-socket))");
    }

    out
}

/// Quote a string as a Scheme-style double-quoted literal, escaping
/// embedded `"` and `\`. Unicode passes through unchanged (Seatbelt
/// accepts UTF-8 paths).
fn quote_path(s: &str) -> String {
    let mut q = String::with_capacity(s.len() + 2);
    q.push('"');
    for c in s.chars() {
        match c {
            '"' => q.push_str("\\\""),
            '\\' => q.push_str("\\\\"),
            other => q.push(other),
        }
    }
    q.push('"');
    q
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::{FilesystemAcl, NetworkAcl, Policy};

    fn make_policy() -> Policy {
        Policy {
            enabled: true,
            filesystem: FilesystemAcl {
                allow_read: vec![PathBuf::from("/")],
                deny_read: vec![PathBuf::from("/Users/u/.ssh")],
                allow_write: vec![PathBuf::from("/Users/u/work"), PathBuf::from("/tmp")],
                deny_write: vec![PathBuf::from("/Users/u/work/.env")],
            },
            network: NetworkAcl {
                allowed_domains: vec!["github.com".into()],
                allow_mach_lookup: vec!["com.apple.distributed_notifications.2".into()],
                ..NetworkAcl::default()
            },
            ..Policy::default()
        }
    }

    #[test]
    fn header_and_default_deny_present() {
        let p = make_policy();
        let s = render_profile(&p);
        assert!(s.contains("(version 1)"));
        assert!(s.contains("(deny default)"));
    }

    #[test]
    fn allow_write_subpaths_rendered() {
        let p = make_policy();
        let s = render_profile(&p);
        assert!(s.contains(r#"(subpath "/Users/u/work")"#));
        assert!(s.contains(r#"(subpath "/tmp")"#));
    }

    #[test]
    fn deny_write_subpaths_rendered() {
        let p = make_policy();
        let s = render_profile(&p);
        assert!(s.contains("(deny file-write*"));
        assert!(s.contains(r#"(subpath "/Users/u/work/.env")"#));
    }

    #[test]
    fn deny_read_subpaths_rendered() {
        let p = make_policy();
        let s = render_profile(&p);
        assert!(s.contains("(deny file-read*"));
        assert!(s.contains(r#"(subpath "/Users/u/.ssh")"#));
    }

    #[test]
    fn mach_lookup_rendered() {
        let p = make_policy();
        let s = render_profile(&p);
        assert!(s.contains(
            "(allow mach-lookup (global-name \"com.apple.distributed_notifications.2\"))"
        ));
    }

    #[test]
    fn network_outbound_rendered_when_allowed_domains_set() {
        let p = make_policy();
        let s = render_profile(&p);
        assert!(s.contains("(allow network-outbound"));
        assert!(s.contains(r#"(remote tcp "github.com:443")"#));
    }

    #[test]
    fn no_network_outbound_when_allowed_domains_empty() {
        let mut p = make_policy();
        p.network.allowed_domains.clear();
        let s = render_profile(&p);
        assert!(!s.contains("(allow network-outbound"));
    }

    #[test]
    fn http_proxy_port_locks_egress_to_loopback() {
        let mut p = make_policy();
        p.network.allowed_domains.clear();
        p.network.http_proxy_port = 8888;
        let s = render_profile(&p);
        assert!(s.contains(r#"(allow network-outbound (remote tcp "127.0.0.1:8888"))"#));
        // No domain-level allows when proxy is set.
        assert!(!s.contains("github.com:443"));
    }

    #[test]
    fn unicode_path_survives_quoting() {
        let mut p = make_policy();
        p.filesystem
            .allow_write
            .push(PathBuf::from("/tmp/café/работа"));
        let s = render_profile(&p);
        assert!(
            s.contains(r#"(subpath "/tmp/café/работа")"#),
            "unicode path corrupted:\n{s}"
        );
    }

    #[test]
    fn quotes_in_path_escaped() {
        let q = quote_path(r#"/tmp/has"quote\bs"#);
        assert_eq!(q, r#""/tmp/has\"quote\\bs""#);
    }

    #[test]
    fn allow_local_binding_emits_network_bind() {
        let mut p = make_policy();
        p.network.allow_local_binding = true;
        let s = render_profile(&p);
        assert!(s.contains(r#"(allow network-bind (local ip "*:0"))"#));
    }
}
