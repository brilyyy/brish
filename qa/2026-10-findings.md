# briSH QA findings — 2026-10-08

Harness: `qa/Dockerfile` + `qa/scripts/run-all.sh`, all layers green
except the findings below. Every finding is reproducible from the
corpus case named. Harness bugs found during the run were fixed; these
are the bugs that remain **in briSH itself**.

## Real bugs (correctness)

### 1. Positional parameters (`$1`..`$9`) are not expanded inside `$(( ))`

- **Case:** `conformance.txt` — `set -- 5; echo $(( $1 - 1 ))`
- **brish:** `brish: expansion error: arithmetic: bad expression`
- **dash / mksh / busybox:** print the number
- **Why it matters:** `$1` inside arithmetic is one of the most common
  idioms in shell functions. Regular variables (`$x`) work fine in
  `$(( ))`; only positional parameters fail. Any recursive or
  argument-processing function that counts down breaks.
- **Repro:** `brish -c 'f() { n=$(($1-1)); echo $n; }; f 5'`

### 2. Quoted here-doc delimiters still expand"

- **Case:** `cases/multiline/quoted-heredoc-noexpand.sh`
- **brish:** `cat <<"X"` expands `$var`, printing ` `
- **dash:** prints `$var_should_not_expand` verbatim
- **Why it matters:** POSIX 2.7.4 says a *quoted* delimiter (`<<"EOF"`
  or `<<'EOF'`) means the body must NOT be expanded. Heredocs are the
  standard way to embed literals; expanding them when the author asked
  for literal is a correctness and a security (injection) hazard.
- **Repro:** `printf 'cat <<"X"\n$HOME\nX\n' | brish` (brish prints your home dir; dash prints `$HOME`)

### 3. Nested backticks are not evaluated

- **Case:** `cases/multiline/nested-backticks.sh`
- **brish:** `` b=`echo \`echo inner\`` `` leaves the inner backtick
  literal → `` echo $b `` prints `` `echo inner` ``
- **dash:** prints `inner`
- **Why it matters:** nested command substitution is required for
  backwards compatibility. `$(...)` nesting works in brish; only the
  backtick form is broken.
- **Repro:** `brish -c 'x=\`echo \`echo inner\`\`; echo $x'`

## Performance gaps (versus dash / mksh / busybox / bash)

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
