# briSH QA harness

Differential, adversarial and performance testing for briSH. Everything
runs inside `qa/Dockerfile` against a release build — the host is never
read from or written to beyond a scratch output directory.

## Run

```sh
docker build -f qa/Dockerfile -t brish-qa .

docker run --rm --cpus 2 --memory 2g --pids-limit 256 \
  -v "$PWD/qa:/qa:ro" -v "$PWD/qa-out:/out" \
  brish-qa /qa/scripts/run-all.sh
```

`--pids-limit` and `--memory` are load-bearing: they are what make the
bounded resource attacks in layer 5 safe. `--rm` leaves nothing behind.

Single layer:

```sh
docker run --rm -v "$PWD/qa:/qa:ro" -v "$PWD/qa-out:/out" \
  brish-qa /qa/scripts/run-all.sh conformance
```

Report lands at `qa-out/report.md`; raw rows at `qa-out/results.<layer>.tsv`.

## Layers

| Script | Layer | Question it answers |
|---|---|---|
| `00-env.sh` | 0 | Are the oracles actually alive? |
| `10-conformance.sh` | 1b | Does briSH match POSIX on stdout **and** exit status? |
| `11-parse-sweep.sh` | 1a | Can it parse the scripts a real Debian ships? |
| `20-features.sh` | 2 | Does every documented feature behave as documented? |
| `21-config-matrix.sh` | 3 | Does a malformed config warn instead of breaking the shell? |
| `22-plugin-store.sh` | 4 | Do store plugins install, gate and time out correctly? |
| `23-interactive.sh` | 6 | Do REPL features work at all? (needs a pty) |
| `30-adversarial.sh` | 5 | Can anything break it — input, env, filenames, signals, resources? |
| `40-bench.sh` | bench | How fast is it, versus four other shells? |
| `43-bench-interactive.sh` | bench | First-prompt and keystroke latency |
| `60-report.sh` | — | Aggregate everything into `report.md` |

## Oracles and why each one is here

| Shell | Role |
|---|---|
| `dash` | Primary. Strictest widely-deployed POSIX shell; `crates/brish/tests/posix.rs` cross-checks against it too. |
| `mksh` | Second POSIX opinion. Catches bugs dash and briSH coincidentally agree on. |
| `busybox ash` | Third POSIX opinion, and a *performance* peer. |
| `bash` | **Non-blocking.** Accepts bash-isms briSH is not required to implement, so a dash mismatch that bash accepts is recorded `DEV`, not `FAIL`. |

The layer-0 script asserts every oracle executes. A silently broken
oracle turns every comparison into "exit 127 = briSH bug", which
manufactures a wall of false findings — `qa/report.md` documents that
this exact confusion cost a debugging round.

## Verdicts

| Verdict | Meaning |
|---|---|
| `PASS` | Matches the oracle / the documentation |
| `FAIL` | Diverges with no justification |
| `DEV` | briSH rejects something another shell accepts; recorded, not failed. `docs/POSIX.md` tracks these |
| `NA` | Nothing to compare — every POSIX oracle rejects it too |
| `SKIP` | Layer could not run (missing optional tool) |

## Two design decisions worth knowing

**stdout and status are compared separately.** Every bug this QA has
found was status-only or silent. A combined comparison hides exactly
that class. Status travels by file, never stdout, because a snippet may
print newlines and corrupt an inline separator.

**Parse failures are gated on `dash -n` first.** When sweeping real
Debian scripts, if dash cannot parse a file either then it is not a
POSIX target — reporting briSH for it is noise, not a bug.

## Corpora

| Path | Format |
|---|---|
| `cases/conformance.txt` | one hermetic program per line |
| `cases/multiline/*.sh` | programs that cannot be one line (here-docs, `f() {` on the next line, nested compounds) |
| `cases/features.txt` | TSV: `name`, `expected_status`, `expected_stdout`, `script` |

Corpora target the bug classes that have actually produced findings,
not a broad POSIX checklist. `qa/report.md` records why: a previous round
had zero `-t`/`-r`/`-w`/`-x` cases and zero mixed-operator list cases,
which is precisely why those four bugs survived to release.

## Adding a case

Conformance — append one line to `cases/conformance.txt`. It must be
hermetic: no clock, no randomness, no paths outside a scratch dir, no
network.

Features — append a TSV row to `cases/features.txt`. `.` in the stdout
column means "don't check" (output is inherently variable). No literal
tabs in the script column.

Adversarial — add a `survive` call to `30-adversarial.sh`. If it should
be *fast*, add it to `cases/adversarial/`; the resource-exhaustion ones
belong inline where their `timeout`/`ulimit` bounds are visible.

## Benchmarks

Methodology: `BENCH_WARMUP` discarded runs, `BENCH_REPEATS` measured
(default 15), reporting the **minimum** — on a shared machine the median
of a noisy sample mostly measures the neighbours. CPU-pinned with
`taskset -c 0` when available. Every shell gets an identical scrubbed
`env -i`.

Gate: briSH's min must be within `BENCH_GATE` (default 2×) of the
*slowest* oracle. Tuning to "no slower than dash" is not realistic for a
shell that also does autosuggest and highlighting, and would make the
gate noise.

Benchmark numbers are comparative **within a single run**. They are not
a regression signal on their own — a different host produces different
numbers. That is why the gate is loose rather than a committed baseline.

## Knobs

| Env | Default | Effect |
|---|---|---|
| `OUT` | `/out` | Output directory |
| `BENCH_REPEATS` | 15 | Measured runs per benchmark point |
| `BENCH_WARMUP` | 3 | Discarded warmup runs |
| `BENCH_GATE` | 2 | Max brish-vs-slowest-oracle ratio before failing |
| `ADVERSARIAL_TIMEOUT` | 20 | Seconds before a runaway script is called a hang |