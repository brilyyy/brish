# briSH Security Model

briSH is a shell: it runs whatever you tell it to run, as you. This
document records the trust boundaries, the hardening that is in place,
and the ceilings deliberately left for later.

## Trust model

| Thing | Trust |
|---|---|
| Commands you type / scripts you source | Trusted — same as `sh` |
| `~/.config/brish/.brishrc`, `config.toml` | Trusted — read and executed as your user |
| Store plugin helpers, segments, hooks | **PATH trust** — running one is running an executable as you. There is **no sandbox** |
| WASM segments (`[wasm]`) | **Sandboxed** — interpreted by wasmi with *no* host imports: no filesystem, no network, no spawn, no clock, no env. A guest that asks for an import fails to instantiate. Bounded by fuel per render and `max_output`; output is stripped of control characters and ESC so it cannot inject prompt escapes |
| Index installs | Commit-pinned when the index entry pins `commit`; mismatch fails closed |
| Completion packs (compiled in) | Data only — no subprocess |

Review third-party plugins before `plugin add`. Prefer commit-pinned
index entries. Helper subprocesses are deadline-killed (default
250ms completion / 500ms segment / 1000ms hook) and their stderr is
discarded, but they are not isolated.

A `[wasm]` segment is the one plugin kind that *is* isolated. The
boundary is the interpreter, so it also costs: it sees only the status
code and cwd the host writes for it, which is why the built-in segments
that need `git` are helpers and not WASM. See
[`PLUGIN-WASM.md`](PLUGIN-WASM.md). Note this is an interpreter, not a
JIT — no host code generation, so no W^X surface.

## File permissions

- `~/.config/brish/` is created `0700` and tightened if it is
  group/world-accessible.
- History (`write_private` path), `config.toml` writes, and the `z`
  database are written `0600`.
- The interactive history file (created by the line editor) is
  tightened to `0600` at REPL startup.
- Here-doc / here-string temps: `O_EXCL`, random name, `0600`, unlink
  on drop.

History is plaintext. It records commands; commands may contain
secrets (`export TOKEN=...`). `history -c` clears it. There is no
redaction filter — the file is yours, mode `0600`.

## Signal traps

`trap` handlers only set a flag (async-signal-safe); trap bodies run
at command/prompt boundaries in the main shell, never inside the
signal handler. `KILL`/`STOP` cannot be trapped.

## Restricted mode

`brish -r` (or `set -o restricted`) is for unprivileged accounts that
need a shell but must not be able to pivot. It blocks:

- `cd` and `z` — no directory changes
- command names containing `/`, and `source` paths containing `/`
- output redirection (`>`, `>>`, `>|`) and `exec`, in both its
  bare-redirection and command-replacing forms
- writes to `SHELL`, `PATH`, `ENV` and `BASH_ENV`

Input redirection (`<`), here-docs and fd duplication (`>&`) stay
available. Once on it cannot be turned off: `set` rejects any `+o` or
`+letter` while restricted.

**Not a security boundary on its own.** It is a speed bump against
casual or accidental escape, not a jail — it constrains the shell's own
syntax, not the programs you can run. Run it with an unprivileged uid,
and remember a restricted shell can still read any file that uid can
read, and still run anything on its `PATH`.

## Known ceilings (deferred)

| Ceiling | Upgrade path |
|---|---|
| No secret redaction in history/debug traces | Opt-in filter over history writes + `BRISH_DEBUG` |
| TOCTOU on path checks (`stat` vs `open`) | `openat`/`execveat` where available |
| Plugin helpers unsandboxed | WASM or OS sandbox — see [`PLUGIN-WASM.md`](PLUGIN-WASM.md) |

## Reporting

Open an issue on the repository. Include the version (`brish
--version`) and a minimal reproduction.
