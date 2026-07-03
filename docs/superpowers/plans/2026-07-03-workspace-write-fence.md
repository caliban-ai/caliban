# `--workspace` fences file writes by default — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `--workspace` imply path-restriction to the workspace root for the file/shell tools, with `--no-restrict-paths` as an explicit opt-out, closing the F2 write-containment gap (#237).

**Architecture:** A single pure predicate `should_restrict(&Args)` decides containment from the parsed flags; `build_registry` consumes it in place of the bare `args.restrict_paths`. A sibling predicate `unfenced_automation(&Args)` drives a startup safety warning when auto-approve runs unfenced. The containment mechanism itself (`WorkspaceRoot::restricted()` / `resolve()`) is unchanged — we only flip when it's turned on.

**Tech Stack:** Rust, clap (derive), the caliban binary crate (`caliban/src/args.rs`, `caliban/src/startup/compose.rs`), `caliban-tools-builtin::WorkspaceRoot`.

## Global Constraints

- Local CI-mirror gate before push: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo build --workspace --all-targets`, `cargo test --workspace`. All must pass.
- Commit author identity for `~/dev/caliban-ai/**`: `john.ford2002@gmail.com`; end commit messages with `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>`.
- Behavior contract: containment default is ON when `--workspace` is set (or `--restrict-paths` given), OFF otherwise; `--no-restrict-paths` forces OFF; `--restrict-paths` + `--no-restrict-paths` is a clap parse error.
- Do not change the no-`--workspace` interactive default (stays unfenced).

---

### Task 1: `--no-restrict-paths` flag + `should_restrict` / `unfenced_automation` predicates

**Files:**
- Modify: `caliban/src/args.rs` — add the flag (near `restrict_paths`, `:214-216`), add two `pub(crate)` predicates (near `resolved_provider`, `:43`), add tests in the `tests` module (`:823+`).

**Interfaces:**
- Produces: `caliban::args::should_restrict(&Args) -> bool` and `caliban::args::unfenced_automation(&Args) -> bool`, plus a new `Args.no_restrict_paths: bool` field.

- [ ] **Step 1: Write the failing tests** in the `tests` module of `caliban/src/args.rs` (the module already has a `fn parse(extra: &[&str]) -> Args` helper that calls `Args::try_parse_from`):

```rust
#[test]
fn should_restrict_truth_table() {
    // workspace alone => restricted (the #237 fix).
    assert!(super::should_restrict(&parse(&["--workspace", "/tmp"])));
    // workspace + explicit opt-out => not restricted.
    assert!(!super::should_restrict(&parse(&[
        "--workspace",
        "/tmp",
        "--no-restrict-paths"
    ])));
    // restrict-paths alone (no workspace) => restricted (unchanged).
    assert!(super::should_restrict(&parse(&["--restrict-paths"])));
    // nothing => interactive default, not restricted.
    assert!(!super::should_restrict(&parse(&[])));
    // opt-out alone => no-op, not restricted.
    assert!(!super::should_restrict(&parse(&["--no-restrict-paths"])));
}

/// #237 F2 regression: the exact leaked scenario — `--workspace B` with no
/// `--restrict-paths` — must now be restricted.
#[test]
fn workspace_without_restrict_paths_is_now_fenced() {
    assert!(super::should_restrict(&parse(&["--workspace", "/some/dir"])));
}

#[test]
fn restrict_and_no_restrict_together_is_a_parse_error() {
    let argv = ["caliban", "--restrict-paths", "--no-restrict-paths"];
    assert!(Args::try_parse_from(argv).is_err());
}

#[test]
fn unfenced_automation_flags_the_danger_combo() {
    // --no-permissions with no fence => flagged.
    assert!(super::unfenced_automation(&parse(&["--no-permissions"])));
    // --no-permissions but fenced via --workspace => not flagged.
    assert!(!super::unfenced_automation(&parse(&[
        "--no-permissions",
        "--workspace",
        "/tmp"
    ])));
    // fenceless but permissioned => not flagged (interactive).
    assert!(!super::unfenced_automation(&parse(&[])));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p caliban should_restrict_truth_table workspace_without_restrict_paths_is_now_fenced restrict_and_no_restrict_together_is_a_parse_error unfenced_automation_flags_the_danger_combo`
Expected: FAIL — no `no_restrict_paths` field, no `should_restrict` / `unfenced_automation` fns.

- [ ] **Step 3: Add the flag.** In `caliban/src/args.rs`, immediately after the `restrict_paths` field (`:216`):

```rust
    /// Opt out of path restriction (the file/shell tools may read and write
    /// outside the workspace root). Restriction is otherwise ON whenever
    /// `--workspace` is set. Conflicts with `--restrict-paths`.
    #[arg(long = "no-restrict-paths", conflicts_with = "restrict_paths")]
    pub(crate) no_restrict_paths: bool,
```

Update the `--workspace` doc comment (`:206`) and `--restrict-paths` doc comment (`:214`) to state the implication:

```rust
    /// Workspace root for file/shell tools. Restricts those tools to this
    /// directory by default (pass `--no-restrict-paths` to opt out).
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,
```
```rust
    /// Reject tool paths outside the workspace root. Implied when
    /// `--workspace` is set; use `--no-restrict-paths` to opt out.
    #[arg(long)]
    pub(crate) restrict_paths: bool,
```

- [ ] **Step 4: Add the predicates.** In `caliban/src/args.rs`, near `resolved_provider` (`:43`):

```rust
/// Whether the file/shell tools should be confined to the workspace root.
///
/// Restriction is ON when `--restrict-paths` is passed **or** a `--workspace`
/// is explicitly chosen (setting a workspace signals intent to scope the agent
/// to it — #237), and OFF when `--no-restrict-paths` overrides. With no
/// workspace and no flags it is OFF (the interactive default).
pub(crate) fn should_restrict(args: &Args) -> bool {
    !args.no_restrict_paths && (args.restrict_paths || args.workspace.is_some())
}

/// Whether this run is auto-approving every tool call **and** leaves the file
/// tools unfenced — the F2 danger combo (#237). Drives a startup warning.
pub(crate) fn unfenced_automation(args: &Args) -> bool {
    args.no_permissions && !should_restrict(args)
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p caliban should_restrict_truth_table workspace_without_restrict_paths_is_now_fenced restrict_and_no_restrict_together_is_a_parse_error unfenced_automation_flags_the_danger_combo`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add caliban/src/args.rs
git commit -m "feat(permissions): --no-restrict-paths flag + should_restrict predicate (#237)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 2: Wire the predicate into `build_registry` + safety warning

**Files:**
- Modify: `caliban/src/startup/compose.rs` — the restrict decision (`:391-395`) and a startup warning.

**Interfaces:**
- Consumes: `crate::args::should_restrict`, `crate::args::unfenced_automation` from Task 1.

- [ ] **Step 1: Replace the restrict decision.** In `caliban/src/startup/compose.rs`, replace `:391-395`:

```rust
    let root = if crate::args::should_restrict(args) {
        workspace.restricted()
    } else {
        // #237: an auto-approve run with no path fence can mutate anywhere on
        // the host. Warn (don't block — the operator may have opted out with
        // --no-restrict-paths on purpose).
        if crate::args::unfenced_automation(args) {
            tracing::warn!(
                target: caliban_common::tracing_targets::TARGET_PERMISSIONS,
                "running with --no-permissions and no path fence: file tools may read/write outside the workspace; pass --workspace (fenced by default) or --restrict-paths to contain them"
            );
        }
        workspace
    };
```

- [ ] **Step 2: Verify build + the existing registry tests still pass**

Run: `cargo build -p caliban && cargo test -p caliban --lib startup::compose`
Expected: PASS (no behavioral test here beyond compilation + existing compose tests; the decision logic is unit-tested in Task 1, and containment itself in Task 3).

- [ ] **Step 3: Commit**

```bash
git add caliban/src/startup/compose.rs
git commit -m "feat(permissions): fence file tools to --workspace by default (#237)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 3: Containment regression test in `caliban-tools-builtin`

**Files:**
- Modify: `crates/caliban-tools-builtin/src/workspace.rs` — add a test asserting a restricted root rejects an out-of-root **write target** (an absolute path outside the root), only if not already covered by `restricted_rejects_outside` (`:194`). Read that test first; if it already asserts an absolute outside-root path is rejected, skip this task and note it.

**Interfaces:**
- Consumes: `WorkspaceRoot::restricted()`, `resolve()`.

- [ ] **Step 1: Read the existing test** `restricted_rejects_outside` (`crates/caliban-tools-builtin/src/workspace.rs:194`). If it already constructs a `restricted()` root and asserts `resolve("<absolute path outside root>")` returns `Err`, this task's coverage exists — record that in the commit/PR and skip to Task 4.

- [ ] **Step 2 (only if not covered): Write the test** in the `tests` module of `crates/caliban-tools-builtin/src/workspace.rs`:

```rust
/// #237: a restricted root must reject a write aimed at an absolute path
/// outside the workspace (the F2 containment guarantee).
#[test]
fn restricted_rejects_absolute_write_outside_root() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = WorkspaceRoot::new(tmp.path()).restricted();
    let err = root
        .resolve("/etc/hosts")
        .expect_err("write outside the fenced root must be rejected");
    assert!(matches!(err, ToolError::InvalidInput(_)), "err: {err:?}");
}
```

- [ ] **Step 3: Run**

Run: `cargo test -p caliban-tools-builtin workspace`
Expected: PASS.

- [ ] **Step 4: Commit (only if a test was added)**

```bash
git add crates/caliban-tools-builtin/src/workspace.rs
git commit -m "test(tools): restricted root rejects absolute out-of-workspace write (#237)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 4: ADR amendment + guide docs

**Files:**
- Create: `docs/adr/00NN-workspace-default-restricted.md` (via the adr-create skill — it assigns the next number).
- Modify: `docs/adr/0010-workspace-root.md` (status line annotation), `docs/adr/README.md` (index row annotation).
- Modify: the guide CLI/permissions reference under `docs/guide/`.

- [ ] **Step 1: Create the amending ADR.** Invoke the **adr-create** skill. Content: title "`--workspace` restricts file/shell tools by default"; Context = the #237 F2 gap (opt-in restriction left auto-approve runs unfenced); Decision = restriction defaults ON when `--workspace` is set, opt out via `--no-restrict-paths`, `--no-permissions`-unfenced warns; Consequences = interactive no-`--workspace` behavior unchanged, `--restrict-paths` now redundant-but-valid with `--workspace`. Mark it as **amending ADR 0010** and have adr-create add the bidirectional annotation to 0010's status + the README index row (the `0005 ← 0042` precedent).

- [ ] **Step 2: Update the guide.** Find the CLI/permissions reference:

Run: `rg -l "restrict-paths|--workspace" docs/guide/src`
In the file(s) that document these flags, state: `--workspace` now confines the file/shell tools to that directory by default; `--no-restrict-paths` opts out; a warning fires under `--no-permissions` without a fence.

- [ ] **Step 3: Commit**

```bash
git add docs/adr docs/guide
git commit -m "docs(permissions): ADR + guide for --workspace default fence (#237)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>"
```

---

### Task 5: Full gate

- [ ] **Step 1: Run the CI-mirror gate**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --all-targets
cargo test --workspace
```
Expected: all pass. `cargo fmt --all` first if the check complains.

- [ ] **Step 2: Straggler grep** — any other `args.restrict_paths` decision sites that should use `should_restrict`:

Run: `rg 'restrict_paths' caliban/src --type rust`
The only *decision* site should be `build_registry`; the field/flag definitions and `should_restrict` internals are expected. Fix any other bare `if args.restrict_paths` that gates path containment.

- [ ] **Step 3: Handoff to cai-ship-it** (sprint Ship step). The diff touches `docs/adr/`, so cai-ship-it will run the adr-validate gate.

## Self-Review

- **Spec coverage:** Component 1 (flag + predicates) → Task 1; Component 2 (registry wiring + warning) → Task 2; testing (truth table, clap conflict, containment, F2) → Tasks 1 + 3; Component 3 (ADR) + Component 4 (docs) → Task 4; gate → Task 5. All mapped.
- **Placeholder scan:** the only deferred specifics are the ADR number (assigned by adr-create) and the exact guide filename (resolved by the `rg` in Task 4 Step 2) — both are resolved by a concrete command/skill, not left vague. Task 3 is explicitly conditional on reading the existing test, with the exact code if needed.
- **Type consistency:** `should_restrict(&Args) -> bool` and `unfenced_automation(&Args) -> bool` are used identically in Tasks 1 and 2; `no_restrict_paths` field name matches `conflicts_with = "restrict_paths"` (the field name of `--restrict-paths`, which clap derives as `restrict_paths`).
