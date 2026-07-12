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
    } else if net.allow_all_outbound {
        // Filesystem-confinement policies (e.g. the `--workspace` Bash fence)
        // keep the network fully open so ordinary commands still work. DNS on
        // macOS is resolved via mDNSResponder over Mach, so a blanket
        // mach-lookup is required alongside the network allow.
        let _ = writeln!(out, ";; Network: unrestricted egress (allow_all_outbound).");
        let _ = writeln!(out, "(allow network*)");
        let _ = writeln!(out, "(allow mach-lookup)");
    }
    // No per-host `allowed_domains` branch: Seatbelt's `(remote tcp …)` filter
    // matches resolved socket addresses, not hostnames, so `(remote tcp
    // "host:443")` cannot express per-host egress — and the child couldn't even
    // resolve the name (no mach-lookup for mDNSResponder in this branch). Rather
    // than emit a rule that looks like it works but doesn't (S9/#408), we emit
    // nothing here: `validate_policy` (#403) already rejects `allowed_domains`
    // without a proxy port, so per-host rules are enforced by the loopback proxy
    // (the `http_proxy_port`/`socks_proxy_port` branches above), never here.
    // With no proxy and no `allow_all_outbound`, `(deny default)` blocks egress.

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
    fn allowed_domains_alone_emit_no_egress_rule() {
        // S9/#408: Seatbelt can't filter egress by hostname, so `allowed_domains`
        // must NOT produce a `(remote tcp "host:443")` rule that looks functional
        // but isn't. `validate_policy` (#403) rejects domains without a proxy, so
        // this configuration never launches; per-host rules are enforced by the
        // proxy branch instead. Here we assert the false affordance is gone.
        let p = make_policy(); // allowed_domains=[github.com], no proxy
        let s = render_profile(&p);
        assert!(
            !s.contains(r#"(remote tcp "github.com:443")"#),
            "false per-host egress rule still emitted:\n{s}"
        );
        assert!(
            !s.contains("(allow network-outbound"),
            "unexpected network-outbound from allowed_domains:\n{s}"
        );
    }

    #[test]
    fn allowed_domains_with_proxy_enforced_via_proxy_not_host_rules() {
        // With a proxy set, egress is locked to loopback and the proxy applies
        // the domain rules — no per-host `(remote tcp host:443)` is emitted.
        let mut p = make_policy();
        p.network.http_proxy_port = 8080;
        let s = render_profile(&p);
        assert!(s.contains(r#"(allow network-outbound (remote tcp "127.0.0.1:8080"))"#));
        assert!(!s.contains(r#"(remote tcp "github.com:443")"#));
    }

    #[test]
    fn no_network_outbound_when_allowed_domains_empty() {
        let mut p = make_policy();
        p.network.allowed_domains.clear();
        let s = render_profile(&p);
        assert!(!s.contains("(allow network-outbound"));
    }

    #[test]
    fn allow_all_outbound_emits_blanket_network_and_mach_lookup() {
        let mut p = make_policy();
        p.network.allowed_domains.clear();
        p.network.allow_all_outbound = true;
        let s = render_profile(&p);
        assert!(s.contains("(allow network*)"), "profile:\n{s}");
        assert!(s.contains("(allow mach-lookup)"), "profile:\n{s}");
    }

    #[test]
    fn proxy_wins_over_allow_all_outbound() {
        // A proxy lock-down must still take precedence over the blanket allow.
        let mut p = make_policy();
        p.network.allowed_domains.clear();
        p.network.allow_all_outbound = true;
        p.network.http_proxy_port = 8888;
        let s = render_profile(&p);
        assert!(s.contains(r#"(allow network-outbound (remote tcp "127.0.0.1:8888"))"#));
        assert!(
            !s.contains("(allow network*)"),
            "blanket allow leaked past proxy:\n{s}"
        );
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
