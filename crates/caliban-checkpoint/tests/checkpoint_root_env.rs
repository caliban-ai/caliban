//! Verifies `CALIBAN_CHECKPOINT_ROOT` overrides the default root.

use std::path::Path;

#[test]
fn root_env_overrides_default() {
    #[allow(unsafe_code)]
    // SAFETY: integration-test binary, single env writer.
    unsafe {
        std::env::set_var("CALIBAN_CHECKPOINT_ROOT", "/tmp/caliban-test-root-XYZ");
    }
    let root = caliban_checkpoint::default_root().unwrap();
    assert_eq!(root, Path::new("/tmp/caliban-test-root-XYZ"));
    #[allow(unsafe_code)]
    // SAFETY: cleanup.
    unsafe {
        std::env::remove_var("CALIBAN_CHECKPOINT_ROOT");
    }
}
