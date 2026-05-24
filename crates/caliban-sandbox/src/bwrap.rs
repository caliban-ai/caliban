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
    // Both deny_read and deny_write are masked with --tmpfs (an empty
    // ramfs shadows the real path). bwrap doesn't distinguish between
    // read-only and read-write denies — masking is the strongest tool
    // it has.
    for p in policy
        .filesystem
        .deny_read
        .iter()
        .chain(policy.filesystem.deny_write.iter())
    {
        let s = path_to_os(p);
        push_str(&mut args, "--tmpfs");
        args.push(s);
    }

    // -- Network -------------------------------------------------------
    let net = &policy.network;
    let allow_proxy = net.http_proxy_port != 0 || net.socks_proxy_port != 0;
    let domains_empty = net.allowed_domains.is_empty();

    if allow_proxy {
        // Deny direct egress; only the operator's loopback proxy is
        // reachable. The proxy enforces domain rules.
        push_str(&mut args, "--unshare-net");
    } else if domains_empty && !net.allow_local_binding {
        push_str(&mut args, "--unshare-net");
    }
    // Otherwise: domains-non-empty without proxy — we keep the network
    // namespace (bwrap can't filter per-hostname). The shim logs a
    // warning at construction time.

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
    fn deny_read_masks_with_tmpfs() {
        let p = Policy {
            filesystem: FilesystemAcl {
                deny_read: vec![PathBuf::from("/home/u/.ssh")],
                ..FilesystemAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(
            contains_pair(&args, "--tmpfs", "/home/u/.ssh"),
            "args = {args:?}",
        );
    }

    #[test]
    fn deny_write_masks_with_tmpfs() {
        let p = Policy {
            filesystem: FilesystemAcl {
                deny_write: vec![PathBuf::from("/work/.env")],
                ..FilesystemAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(
            contains_pair(&args, "--tmpfs", "/work/.env"),
            "args = {args:?}",
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
    fn allowed_domains_keep_net_namespace() {
        // With domains set and no proxy, bwrap can't enforce per-hostname —
        // we keep the namespace and rely on the caller's warning.
        let p = Policy {
            network: NetworkAcl {
                allowed_domains: vec!["github.com".into()],
                ..NetworkAcl::default()
            },
            ..Policy::default()
        };
        let args = build_args(&p);
        assert!(!args.contains(&os("--unshare-net")), "args = {args:?}");
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
