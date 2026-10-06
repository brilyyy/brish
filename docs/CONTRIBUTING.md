# Contributing to briSH

## Gates (must be green on every change)

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Tests: ~16 suites, 350+. The POSIX suite cross-checks every case
against `/bin/dash` when present — keep it that way when adding cases.

## Zero-panic policy

- `clippy::unwrap_used` / `expect_used` are **denied** outside tests
  (workspace lints). No `unwrap` on user input, I/O, or signals.
- `unsafe` lives only in `brish-platform`, each block `// SAFETY:`
  commented. Every other crate is `#![forbid(unsafe_code)]`.
- Add `Result` propagation; non-trivial logic leaves one runnable check.

## Testing traps

- Signal-handler tests write flags via `inject_pending_trap` or real
  self-signals — do **not** restore `SIG_DFL` while a self-signal may
  still be pending (it kills the suite when delivery lands late).
  Keep `TRAP_LOCK` serialization for tests that install/reset handlers.
- `FdScope`/`write_stdout` tests: assert content, not byte-exact output
  (the harness writes fd1 lines into the redirect window).

## Fuzz

```sh
cargo +nightly fuzz run lexer     # parser, expand
```

Run before touching the lexer/parser/expander.

## Style

- `cargo fmt`; short commits in Conventional Commits (`feat:`, `fix:`,
  `refactor(test|docs):`).
- Terse `ponytail:` comments name a deliberate simplification and its
  upgrade path (never delete a shortcut silently).
- Keep `brish-plugin`/`brish-theme` engine-free; add catalog entries to
  `catalog()` with config-gating.

## Docs

- `docs/PLUGINS.md` — extension authoring (authoritative API).
- `docs/CONFIGURATION.md` — config reference.
- `docs/POSIX.md` — conformance matrix, coverage, latency, fuzz.
- `docs/SECURITY.md` — trust model + ceilings.
- `docs/ARCHITECTURE.md` — component map.

New configuration knobs go in `config.rs` (+ `ucfg.rs` for engine-side)
with tests; update `CONFIGURATION.md` and the CHANGELOG.