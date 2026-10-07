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
| Index installs | Commit-pinned when the index entry pins `commit`; mismatch fails closed |
| Completion packs (compiled in) | Data only — no subprocess |

Review third-party plugins before `plugin add`. Prefer commit-pinned
index entries. Helper subprocesses are deadline-killed (default
250ms completion / 500ms segment / 1000ms hook) and their stderr is
discarded, but they are not isolated.

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

## Known ceilings (deferred)

| Ceiling | Upgrade path |
|---|---|
| No restricted mode (`bsh -r`) | Wire `ModePolicy::strict_posix` into exec |
| No secret redaction in history/debug traces | Opt-in filter over history writes + `BRISH_DEBUG` |
| TOCTOU on path checks (`stat` vs `open`) | `openat`/`execveat` where available |
| Plugin helpers unsandboxed | WASM or OS sandbox — see [`PLUGIN-WASM.md`](PLUGIN-WASM.md) |

## Reporting

Open an issue on the repository. Include the version (`brish
--version`) and a minimal reproduction.
