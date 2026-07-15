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
            ";; Network: HTTP proxy at localhost:{}.",
            net.http_proxy_port
        );
        // `localhost`, not `127.0.0.1`: Seatbelt rejects a literal IP ("host must
        // be * or localhost in network address") and an invalid rule makes it
        // refuse the entire profile, so every sandboxed command fails to launch.
        // This shipped broken — the proxy posture was never exercised on macOS.
        let _ = writeln!(
            out,
            "(allow network-outbound (remote ip \"localhost:{}\"))",
            net.http_proxy_port
        );
    } else if net.socks_proxy_port != 0 {
        let _ = writeln!(
            out,
            ";; Network: SOCKS proxy at localhost:{}.",
            net.socks_proxy_port
        );
        // See the http_proxy_port branch: `localhost`, never a literal IP.
        let _ = writeln!(
            out,
            "(allow network-outbound (remote ip \"localhost:{}\"))",
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
    } else if net.allow_local_binding {
        // Loopback only (#406). The child may bind and connect to 127.0.0.1 —
        // test servers, dev servers, suites that spin up a listener — but has no
        // route off the box.
        //
        // This exists because the two backends are NOT symmetric. On Linux,
        // `--unshare-net` gives loopback for free: the isolated netns has `lo`
        // up. Seatbelt is `(deny default)`, so emitting no network rule denies
        // loopback along with egress. Without this branch, closing egress would
        // break every localhost test suite on macOS and nowhere else.
        //
        // No `mach-lookup`: DNS resolution goes through mDNSResponder over Mach,
        // and a loopback-only child has no business resolving public names.
        //
        // SBPL note: the host part of a network address must be literally `*` or
        // `localhost` — `127.0.0.1` is rejected ("host must be * or localhost"),
        // and the port must be a number or `*` (`0` is rejected as an invalid
        // port). Both mistakes make `sandbox-exec` refuse the whole profile, so
        // every sandboxed command fails to launch. `profile_compiles_*` tests
        // below run the generated profile through the real `sandbox-exec`.
        let _ = writeln!(out, ";; Network: loopback only (allow_local_binding).");
        let _ = writeln!(out, r#"(allow network-outbound (remote ip "localhost:*"))"#);
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
        // Was `(local ip "*:0")`, which Seatbelt rejects outright — port `0` is
        // "an invalid port in network address", and an invalid rule makes
        // `sandbox-exec` refuse the ENTIRE profile, so every sandboxed command
        // fails to launch. It never fired only because nothing set
        // `allow_local_binding` in production; the #406 fence sets it, so it
        // would have broken every macOS Bash command under `--workspace`.
        let _ = writeln!(out, ";; Allow local bind (loopback).");
        let _ = writeln!(out, r#"(allow network-bind (local ip "localhost:*"))"#);
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

    /// Render `policy` and hand the profile to the real `sandbox-exec`. Returns
    /// the error text when Seatbelt refuses to compile it.
    ///
    /// This is the check the string-matching tests below cannot make. A profile
    /// can contain exactly the text we expect and still be *invalid* — and an
    /// invalid rule makes `sandbox-exec` reject the WHOLE profile, so every
    /// sandboxed command fails to launch. That is how `(local ip "*:0")` sat in
    /// this file unnoticed: nothing set `allow_local_binding`, so it never ran.
    #[cfg(target_os = "macos")]
    fn compile_error(policy: &Policy) -> Option<String> {
        use std::io::Write as _;
        let text = render_profile(policy);
        let mut f = tempfile::Builder::new()
            .suffix(".sb")
            .tempfile()
            .expect("tempfile");
        f.write_all(text.as_bytes()).expect("write");
        let out = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-f")
            .arg(f.path())
            .args(["/bin/sh", "-c", "exit 0"])
            .output()
            .expect("spawn sandbox-exec");
        if out.status.success() {
            None
        } else {
            Some(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn profile_compiles_with_loopback_posture() {
        // The shipped #406 fence: reads open, egress closed, loopback up.
        let p = Policy {
            enabled: true,
            filesystem: FilesystemAcl {
                allow_read: vec![PathBuf::from("/")],
                allow_write: vec![PathBuf::from("/tmp")],
                ..FilesystemAcl::default()
            },
            network: NetworkAcl {
                allow_all_outbound: false,
                allow_local_binding: true,
                ..NetworkAcl::default()
            },
            ..Policy::default()
        };
        assert_eq!(
            compile_error(&p),
            None,
            "sandbox-exec must accept the fence profile — an invalid rule makes \
             it reject the whole profile, breaking every sandboxed command"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn profile_compiles_for_every_network_posture() {
        for (label, net) in [
            ("closed", NetworkAcl::default()),
            (
                "loopback",
                NetworkAcl {
                    allow_local_binding: true,
                    ..NetworkAcl::default()
                },
            ),
            (
                "open",
                NetworkAcl {
                    allow_all_outbound: true,
                    ..NetworkAcl::default()
                },
            ),
            (
                "proxy",
                NetworkAcl {
                    http_proxy_port: 8888,
                    ..NetworkAcl::default()
                },
            ),
            (
                "unix-sockets",
                NetworkAcl {
                    allow_unix_sockets: true,
                    allow_local_binding: true,
                    ..NetworkAcl::default()
                },
            ),
        ] {
            let p = Policy {
                enabled: true,
                filesystem: FilesystemAcl {
                    allow_read: vec![PathBuf::from("/")],
                    ..FilesystemAcl::default()
                },
                network: net,
                ..Policy::default()
            };
            assert_eq!(
                compile_error(&p),
                None,
                "sandbox-exec rejected the '{label}' network posture"
            );
        }
    }

    #[test]
    fn local_binding_allows_loopback_only() {
        // Seatbelt is `(deny default)`, so with no network rule even loopback is
        // denied — unlike bwrap's --unshare-net, which keeps `lo` up inside the
        // isolated netns. Without an explicit loopback branch, closing egress
        // (#406) would also kill every localhost test/dev server, on macOS only.
        let p = Policy {
            enabled: true,
            network: NetworkAcl {
                allow_local_binding: true,
                allow_all_outbound: false,
                ..NetworkAcl::default()
            },
            ..Policy::default()
        };
        let s = render_profile(&p);
        assert!(
            s.contains(r#"(allow network-outbound (remote ip "localhost:*"))"#),
            "loopback egress must be permitted when allow_local_binding is set:\n{s}"
        );
        assert!(
            s.contains(r#"(allow network-bind (local ip "localhost:*"))"#),
            "loopback bind must be permitted when allow_local_binding is set:\n{s}"
        );
        assert!(
            !s.contains("(allow network*)"),
            "must NOT emit a blanket network allow — that is full egress:\n{s}"
        );
    }

    #[test]
    fn no_network_flags_denies_all_network() {
        let s = render_profile(&Policy::default());
        assert!(
            !s.contains("(allow network"),
            "a default policy must emit no network allow at all:\n{s}"
        );
    }

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
        assert!(s.contains(r#"(allow network-outbound (remote ip "localhost:8080"))"#));
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
        assert!(s.contains(r#"(allow network-outbound (remote ip "localhost:8888"))"#));
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
        assert!(s.contains(r#"(allow network-outbound (remote ip "localhost:8888"))"#));
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
        // This used to assert `(local ip "*:0")` — a rule Seatbelt *rejects*
        // ("invalid port in network address"), which makes it refuse the entire
        // profile. The test pinned an unusable profile in place; it passed only
        // because it string-matched the generated text and never asked
        // sandbox-exec whether the profile was valid. See
        // `profile_compiles_for_every_network_posture`, which does.
        let mut p = make_policy();
        p.network.allow_local_binding = true;
        let s = render_profile(&p);
        assert!(s.contains(r#"(allow network-bind (local ip "localhost:*"))"#));
    }
}
