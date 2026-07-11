//! Linux bubblewrap (`bwrap`) argv generation.

use std::ffi::OsString;
use std::path::Path;

use crate::config::Policy;

/// Build the argv (excluding `bwrap` itself) for invoking the wrapped
/// command. The caller appends `--` followed by the original program +
/// args.
#[must_use]
pub fn build_args(policy: &Policy) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();

    // Survival flags: die with parent, isolate session / namespaces.
    push_str(&mut args, "--die-with-parent");
    push_str(&mut args, "--new-session");

    if policy.enable_weaker_nested_sandbox {
        // Already in a user namespace (dev container): skip --unshare-user
        // and --unshare-cgroup-try which would otherwise fail.
        push_str(&mut args, "--unshare-pid");
        push_str(&mut args, "--unshare-ipc");
    } else {
        push_str(&mut args, "--unshare-user");
        push_str(&mut args, "--unshare-pid");
        push_str(&mut args, "--unshare-ipc");
        push_str(&mut args, "--unshare-cgroup-try");
    }

    // Mount /proc, /dev, and a fresh /tmp inside the sandbox.
    push_pair(&mut args, "--proc", "/proc");
    push_pair(&mut args, "--dev", "/dev");
    push_str(&mut args, "--tmpfs");
    args.push("/tmp".into());

    // -- Reads ---------------------------------------------------------
    // System read-only roots that the sandboxed process effectively
    // always needs to exec a /bin/sh -c command (libc, /etc, /bin).
    // Only bind those that actually exist (so building on minimal
    // sandboxes-of-sandboxes doesn't fail). The policy's explicit
    // allow_read entries take precedence and are *always* attempted.
    for sysroot in ["/usr", "/etc", "/bin", "/lib", "/lib64", "/opt"] {
        if Path::new(sysroot).exists() {
            push_ro_bind(&mut args, sysroot, sysroot);
        }
    }
    for p in &policy.filesystem.allow_read {
        let s = path_to_os(p);
        push_pair_os(&mut args, "--ro-bind-try", &s, &s);
    }

    // -- Writes --------------------------------------------------------
    for p in &policy.filesystem.allow_write {
        let s = path_to_os(p);
        push_pair_os(&mut args, "--bind-try", &s, &s);
    }

    // -- Denies (masks) ------------------------------------------------
    // #407: mask precisely. `--tmpfs` only mounts over a *directory*, so masking
    // a file (the common secret case: ~/.aws/credentials, .env, id_rsa) made
    // bwrap abort at startup; and a tmpfs is a fresh *writable* ramfs, so it
    // didn't actually prevent writes — it silently discarded them.
    //   - deny_read hides content: `--ro-bind /dev/null` for a file (empty,
    //     read-only) or `--tmpfs` for a directory (empty view).
    //   - deny_write keeps content readable but blocks writes: a read-only bind
    //     of the path over itself (writes fail EROFS), for files and dirs alike.
    // `-try` variants tolerate a missing path (nothing to hide) without aborting.
    let dev_null = OsString::from("/dev/null");
    for p in &policy.filesystem.deny_read {
        let s = path_to_os(p);
        if p.is_dir() {
            push_str(&mut args, "--tmpfs");
            args.push(s);
        } else {
            push_pair_os(&mut args, "--ro-bind-try", &dev_null, &s);
        }
    }
    for p in &policy.filesystem.deny_write {
        let s = path_to_os(p);
        push_pair_os(&mut args, "--ro-bind-try", &s, &s);
    }

    // -- Network -------------------------------------------------------
    let net = &policy.network;
    let allow_proxy = net.http_proxy_port != 0 || net.socks_proxy_port != 0;

    // Isolate the network unless egress is *explicitly* required. A bare
    // `allowed_domains`/`denied_domains` list does NOT keep the namespace open:
    // bwrap can't filter per-hostname, so those lists are enforceable only via
    // the proxy (`validate_policy` rejects them otherwise). Keeping the network
    // open for a domain list would grant ALL egress — the inversion #403 fixes.
    if allow_proxy {
        // Deny direct egress; only the operator's loopback proxy is reachable.
        // The proxy enforces domain rules.
        push_str(&mut args, "--unshare-net");
    } else if !net.allow_all_outbound && !net.allow_local_binding {
        push_str(&mut args, "--unshare-net");
    }
    // Otherwise (`allow_all_outbound` or `allow_local_binding`): keep the
    // network namespace so egress works.

    if !net.allow_unix_sockets {
        // Default: hide the most common host sockets so accidental
        // access doesn't leak. (Operators allow_unix_sockets=true when
        // they explicitly want Docker etc.)
        // We do this by simply not binding /var/run.
    }

    args
}

fn push_str(args: &mut Vec<OsString>, s: &str) {
    args.push(s.into());
}

fn push_pair(args: &mut Vec<OsString>, flag: &str, value: &str) {
    args.push(flag.into());
    args.push(value.into());
}

fn push_ro_bind(args: &mut Vec<OsString>, src: &str, dst: &str) {
    // Use --ro-bind-try (vs --ro-bind) so the sandbox doesn't fail when
    // a path is missing — important for cross-distro robustness.
    args.push("--ro-bind-try".into());
    args.push(src.into());
    args.push(dst.into());
}

fn push_pair_os(args: &mut Vec<OsString>, flag: &str, src: &OsString, dst: &OsString) {
    args.push(flag.into());
    args.push(src.clone());
    args.push(dst.clone());
}

#[cfg(unix)]
fn path_to_os(p: &Path) -> OsString {
    p.as_os_str().to_owned()
}

#[cfg(not(unix))]
fn path_to_os(p: &Path) -> OsString {
    p.as_os_str().to_owned()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::{FilesystemAcl, NetworkAcl, Policy};

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    fn contains_pair(args: &[OsString], flag: &str, value: &str) -> bool {
        args.windows(2)
            .any(|w| w[0] == os(flag) && w[1] == os(value))
    }

    fn contains_triple(args: &[OsString], flag: &str, a: &str, b: &str) -> bool {
        args.windows(3)
            .any(|w| w[0] == os(flag) && w[1] == os(a) && w[2] == os(b))
    }

    #[test]
    fn default_includes_die_with_parent_and_unshares() {
        let p = Policy::default();
        let args = build_args(&p);
        assert!(args.contains(&os("--die-with-parent")));
        assert!(args.contains(&os("--new-session")));
        assert!(args.contains(&os("--unshare-user")));
        assert!(args.contains(&os("--unshare-pid")));
        assert!(args.contains(&os("--unshare-ipc")));
        // Default has no allowed_domains and no local binding → net unshared.
        assert!(args.contains(&os("--unshare-net")));
    }

    #[test]
    fn allow_read_emits_ro_bind_try() {
        let p = Policy {
            filesystem: FilesystemAcl {
                allow_read: vec![PathBuf::from("/etc/foo")],
                ..FilesystemAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(
            contains_triple(&args, "--ro-bind-try", "/etc/foo", "/etc/foo"),
            "args = {args:?}",
        );
    }

    #[test]
    fn allow_write_emits_bind_try() {
        let p = Policy {
            filesystem: FilesystemAcl {
                allow_write: vec![PathBuf::from("/work")],
                ..FilesystemAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(
            contains_triple(&args, "--bind-try", "/work", "/work"),
            "args = {args:?}",
        );
    }

    #[test]
    fn deny_read_file_shadows_with_dev_null() {
        // #407: a *file* deny_read must use --ro-bind /dev/null (a --tmpfs would
        // abort bwrap, since tmpfs only mounts over a directory).
        let tmp = tempfile::tempdir().unwrap();
        let secret = tmp.path().join("credentials");
        std::fs::write(&secret, "token").unwrap();
        let secret_str = secret.to_str().unwrap().to_owned();
        let p = Policy {
            filesystem: FilesystemAcl {
                deny_read: vec![secret],
                ..FilesystemAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(
            contains_pair(&args, "--ro-bind-try", "/dev/null"),
            "file deny_read should shadow with /dev/null; args = {args:?}",
        );
        assert!(
            args.iter().any(|a| *a == os(&secret_str)),
            "the denied path must appear as the bind destination; args = {args:?}",
        );
    }

    #[test]
    fn deny_read_dir_masks_with_tmpfs() {
        // A *directory* deny_read still uses --tmpfs (an empty dir view).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("secrets");
        std::fs::create_dir(&dir).unwrap();
        let dir_str = dir.to_str().unwrap().to_owned();
        let p = Policy {
            filesystem: FilesystemAcl {
                deny_read: vec![dir],
                ..FilesystemAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(
            contains_pair(&args, "--tmpfs", &dir_str),
            "dir deny_read should use --tmpfs; args = {args:?}",
        );
    }

    #[test]
    fn deny_write_is_readonly_self_bind() {
        // #407: deny_write must keep content readable but block writes → a
        // read-only bind of the path over itself, NOT a writable tmpfs that
        // silently discards writes.
        let p = Policy {
            filesystem: FilesystemAcl {
                deny_write: vec![PathBuf::from("/work/.env")],
                ..FilesystemAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(
            contains_pair(&args, "--ro-bind-try", "/work/.env"),
            "deny_write should be a read-only self-bind; args = {args:?}",
        );
        assert!(
            !contains_pair(&args, "--tmpfs", "/work/.env"),
            "deny_write must not use a writable tmpfs; args = {args:?}",
        );
    }

    #[test]
    fn unshare_user_dropped_when_nested() {
        let p = Policy {
            enable_weaker_nested_sandbox: true,
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(!args.contains(&os("--unshare-user")));
        // pid / ipc remain (they don't require user-namespace nesting fix).
        assert!(args.contains(&os("--unshare-pid")));
    }

    #[test]
    fn bare_allowed_domains_isolate_net_not_open_it() {
        // #403: a domain list without a proxy must NOT keep the namespace open
        // (bwrap can't filter per-host; opening it would grant ALL egress, an
        // inversion). It isolates the network instead. (In production
        // `validate_policy` rejects this config outright; build_args stays safe
        // regardless.)
        let p = Policy {
            network: NetworkAcl {
                allowed_domains: vec!["github.com".into()],
                ..NetworkAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(args.contains(&os("--unshare-net")), "args = {args:?}");
    }

    #[test]
    fn http_proxy_forces_unshare_net() {
        let p = Policy {
            network: NetworkAcl {
                allowed_domains: vec!["github.com".into()],
                http_proxy_port: 8888,
                ..NetworkAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(args.contains(&os("--unshare-net")));
    }

    #[test]
    fn allow_all_outbound_keeps_net_namespace() {
        let p = Policy {
            network: NetworkAcl {
                allow_all_outbound: true,
                ..NetworkAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(!args.contains(&os("--unshare-net")), "args = {args:?}");
    }

    #[test]
    fn proxy_still_unshares_net_despite_allow_all_outbound() {
        let p = Policy {
            network: NetworkAcl {
                allow_all_outbound: true,
                http_proxy_port: 8888,
                ..NetworkAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(args.contains(&os("--unshare-net")));
    }

    #[test]
    fn local_binding_keeps_net_namespace() {
        let p = Policy {
            network: NetworkAcl {
                allow_local_binding: true,
                ..NetworkAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(!args.contains(&os("--unshare-net")));
    }

    #[test]
    fn unicode_path_in_allow_write_preserved() {
        let p = Policy {
            filesystem: FilesystemAcl {
                allow_write: vec![PathBuf::from("/tmp/café/работа")],
                ..FilesystemAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        let needle = os("/tmp/café/работа");
        assert!(args.iter().any(|a| a == &needle), "args = {args:?}");
    }
}
