# briSH QA findings — 2026-10-08

> **Status: all findings below are FIXED.** The suites are green (448 pass /
> 0 fail). This file is kept as the record of the 2026-10-08 run, so the
> sections read *pre-fix*: each bug keeps its original symptom because that
> is what the run observed. Resolutions, commit refs and the current
> benchmark table are in [Status after fix pass](#status-after-fix-pass-2026-10)
> at the bottom. Do not read the tables in the middle of this file as the
> current state of briSH.

Harness: `qa/Dockerfile` + `qa/scripts/run-all.sh`, all layers green
except the findings below. Every finding is reproducible from the
corpus case named. Harness bugs found during the run were fixed; these
are the bugs that remain **in briSH itself** (as of that run — all since
fixed, see the status note above).

## Real bugs (correctness)

### 1. [FIXED `7406e36`] Positional parameters (`$1`..`$9`) are not expanded inside `$(( ))`

- **Case:** `conformance.txt` — `set -- 5; echo $(( $1 - 1 ))`
- **brish:** `brish: expansion error: arithmetic: bad expression`
- **dash / mksh / busybox:** print the number
- **Why it matters:** `$1` inside arithmetic is one of the most common
  idioms in shell functions. Regular variables (`$x`) work fine in
  `$(( ))`; only positional parameters fail. Any recursive or
  argument-processing function that counts down breaks.
- **Repro:** `brish -c 'f() { n=$(($1-1)); echo $n; }; f 5'`

### 2. [FIXED `677b605`] Quoted here-doc delimiters still expand"

- **Case:** `cases/multiline/quoted-heredoc-noexpand.sh`
- **brish:** `cat <<"X"` expands `$var`, printing ` `
- **dash:** prints `$var_should_not_expand` verbatim
- **Why it matters:** POSIX 2.7.4 says a *quoted* delimiter (`<<"EOF"`
  or `<<'EOF'`) means the body must NOT be expanded. Heredocs are the
  standard way to embed literals; expanding them when the author asked
  for literal is a correctness and a security (injection) hazard.
- **Repro:** `printf 'cat <<"X"\n$HOME\nX\n' | brish` (brish prints your home dir; dash prints `$HOME`)

### 3. [FIXED `d2632c9`] Nested backticks are not evaluated

- **Case:** `cases/multiline/nested-backticks.sh`
- **brish:** `` b=`echo \`echo inner\`` `` leaves the inner backtick
  literal → `` echo $b `` prints `` `echo inner` ``
- **dash:** prints `inner`
- **Why it matters:** nested command substitution is required for
  backwards compatibility. `$(...)` nesting works in brish; only the
  backtick form is broken.
- **Repro:** `brish -c 'x=\`echo \`echo inner\`\`; echo $x'`

## Performance gaps (versus dash / mksh / busybox / bash)

> **[SUPERSEDED]** Every number in this section is pre-fix. All of them
> traced back to two bugs (`other_threads_alive()` field drift, and a
> full `Env` clone per expansion) that are resolved — see
> [Status after fix pass](#status-after-fix-pass-2026-10) for the current
> table. The `parse_big 45x`, `subshell 34x`, `fork 21x` and `pipeline
> 20x` rows below are **not** what brish does today.

min-of-15 on a shared container, CPU-pinned. "—" = brish equals or beats
the others. These are real and reproducible; they are the kind of thing
to optimize against a real workload, not a bug.

| Workload | dash | mksh | busybox | bash | brish | gap |
|---|---:|---:|---:|---:|---:|---|
| startup | 0 | 0 | 0 | 1 | 1 | ~1x |
| small | 0 | 0 | 0 | 1 | 1 | ~1x |
| expansion | 1 | 2 | 1 | 3 | 1 | — |
| case | 1 | 1 | 1 | 2 | 4 | ~2x |
| arith | 1 | 2 | 1 | 2 | 4 | ~2x |
| function | 2 | 4 | 3 | 7 | 15 | ~2x |
| glob | 11 | 14 | 10 | 16 | 32 | ~2x |
| builtins | 2 | 3 | 2 | 4 | 11 | ~3x |
| parse_big (2000 assigns) | 1 | 2 | 1 | 2 | 90 | ~45x |
| fork (200 /bin/true) | 18 | 26 | 22 | 28 | 599 | ~21x |
| pipeline (60x 3-stage) | 20 | 28 | 13 | 27 | 560 | ~20x |
| subshell (300x `(x=i)`) | 12 | 20 | 14 | 26 | 901 | ~34x |

The three worst — **parse_big 45x**, **subshell 34x**, **fork 21x**,
**pipeline 20x** — cluster around spawning and spawning children. Each
pipeline stage / subshell / fork in brish costs roughly what dash pays
for one, suggesting brish is doing per-fork work that other shells do
once: full config reload, plugin re-scan, or a fresh `Engine` instead of
an inherited state. That is the first thing to profile.

Startup and expansion are at parity, so the gap is not "Rust is slow at
these workloads" in general — it is concentrated in the exec path.

### Resolution: root cause was `other_threads_alive()` field drift

The four exec-path findings were a **single bug**, not four.
`crates/brish-platform/src/proc.rs::other_threads_alive()` parsed
`/proc/self/stat` by reading token index 1 after the last `)` — that is
**ppid**, not `num_threads` (which is index 17). So the function
returned `ppid > 1`, i.e. true whenever brish had *any* parent other
than PID 1. Under a forking wrapper (`timeout`, `sudo`, a non-exec
`sh -c`) every external spawn fell into the 2 ms `WAIT_POLL` path in
`wait_pid`/`wait_untraced`. Direct under the container init (ppid 1)
used the blocking wait and looked fine, which masked it everywhere
except `timeout`.

One-line fix (`nth(1)` → `nth(17)`): the spawn-path findings collapse from
~20-34x to 1.3-1.6x. Post-fix brish vs dash (ms, min of 15):

| fixture | dash | brish | ratio |
|---|---|---|---|
| fork | 18 | 24 | 1.3x |
| pipeline | 19 | 31 | 1.6x |
| subshell | 13 | 21 | 1.6x |

The interactive/multi-threaded poll path still engages correctly because
reedline really does spawn helper threads.

An earlier revision of this file claimed brish was *faster* than dash on
every fixture, including `parse_big 1 ms = dash`. That was measured
against a macOS (Mach-O) binary bind-mounted into the Linux QA image,
which failed to exec — the fast numbers were exec-failure latency, not
work done. Corrected above against `qa-out/bench/results.tsv`.

## Non-findings (verified clean)

- Layer 0: all 5 oracles execute.
- Layer 3 config matrix: 26/26 — malformed TOML, unknown keys, wrong
  types, duplicate keys all produce warnings + defaults, never a crash.
- Layer 4 plugin store: 17/17 (3 skips = missing examples in image) —
  install, gate, purge, deadline-kill for hanging/flooding helpers.
- Layer 5 adversarial: 64/64 — malformed input, hostile env vars,
  injection filenames (`$(touch pwned)` as a filename does not execute),
  10k+ redirection fd-leak check, bounded fork bomb, disk fill, memory
  hog, SIGKILL config integrity all survive.
- Layer 6 interactive: 9/10 — REPL comes up, runs commands, Ctrl-C
  recovers, NO_COLOR strips ANSI, theme/PS1 env respected (1 skip =
  ANSI control was vacuous under the bare pty).

## Harness notes / known gaps

- Interactive assertions are **liveness** (shell came up and answered),
  not pixel-truth. A real terminal emulator (pyte) in the image would
  let the interactive layer assert on what is actually rendered.
- `docs/POSIX.md` already records the `${x:1}` and `2**3` bash-ism
  deviations; the harness now classifies those as `DEV`, matching the
  doc.
- The three bash-ism rejections above are the only POSIX-level
  divergences between brish and dash that the corpus exercises.

## Status after fix pass (2026-10)

- **[FIXED]** `other_threads_alive()` parsed token 1 (ppid) as `num_threads`
  (index 17) after `)`. Under any forking wrapper (`timeout`, `sudo`,
  non-exec `sh -c`) every external spawn paid the 2ms `WAIT_POLL` path.
  Exec-path findings dropped from ~20-34x to 1.3-1.6x. Commit `78c440f`.
- **[FIXED]** `$1`/`$n`/`${n}` inside `$(( ))`: arith rejected `$`. Expander
  now inlines param values. Commit `7406e36`.
- **[FIXED]** quoted heredoc delim (`<<"E"OF`, backslash) still expanded the
  body. `word_contains_single` → `word_quoted` (Single|Double|Esc).
  Commit `677b605`.
- **[FIXED]** nested backticks / escaped `\$` inside backticks kept the
  backslash, so inner didn't evaluate. `read_backtick` now strips the escape
  for `` ` ``, `$`, `\` per POSIX phase 1. Commit `d2632c9`.
- **[FIXED]** `xexpand` deep-cloned the whole `Env` on *every* expansion to
  have a snapshot available for `$(...)`. That made each expansion O(vars):
  one `xN=N` assignment cost 16us at 0 vars but 126us at 1600 vars — quadratic
  over a script. The clone now happens only when the word actually contains a
  command substitution (`word_has_subst`). **parse_big 93ms → 2ms** (dash 1ms).
- **[FIXED]** `has_meta` treated a bare `[` as a glob metacharacter, so the
  command word `[` in every `[ ... ]` test ran a `read_dir` of the cwd —
  6 syscalls per loop iteration (`builtins` fixture). An unterminated bracket
  expression cannot match, so it is literal. **builtins 9ms → 5ms**
  (dash 2ms); `function` 12ms → 8ms (dash 3ms).

All bench gate-fails now clear; suite is 448 pass / 0 fail.

### Final bench (ms, min of 15; brish vs dash)

| fixture | dash | brish | ratio |
|---|---|---|---|
| parse_big | 1 | 2 | 2x |
| builtins | 2 | 5 | 2.5x |
| function | 3 | 8 | 2.7x |
| subshell | 13 | 19 | 1.5x |
| fork | 18 | 23 | 1.3x |
| pipeline | 19 | 31 | 1.6x |

The residual 1.3-2.7x is ordinary interpreter overhead (Rust vs the C
oracles); the gate allows 2x the slowest oracle and brish now meets it
throughout. `qa-out/bench/gate.txt` is empty.

Repro: `qa/scripts/run-all.sh bench`, then read `qa-out/bench/gate.txt`.

### Independent re-check (2026-10-08, after the fix pass)

The three correctness repros, run on a linux release build (ubuntu 24.04)
against both brish and the dash oracle — they agree on all three now:

| repro | brish | dash |
|---|---|---|
| `f() { n=$(($1-1)); echo $n; }; f 5` | `4` | `4` |
| `cat <<"X"` with `$HOME` in the body | `$HOME` | `$HOME` |
| nested `` echo `echo inner` `` | `inner` | `inner` |

Fork and assignment cost, measured outside the harness on that same
release build (min of 5, seconds) — independent of `qa-out/bench`:

| workload | brish | dash | bash |
|---|---|---|---|
| 200 × `/bin/true` | 0.05 | 0.04 | 0.06 |
| 2000 × `xN=N` | 0.02 | 0.02 | 0.47 |

~1.25x dash on forks, parity on assignments: consistent with the 1.3x
post-fix `fork` row above. Two measurement traps are worth remembering
before quoting any number from this file: quote **release** builds
(`target/debug` brish is ~2x slower on these workloads), and do not
cross-measure a binary from one platform against oracles on another —
that mistake is recorded under "Resolution" above.
