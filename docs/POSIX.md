# POSIX conformance & stability measurements

## Conformance harness

`crates/brish/tests/posix.rs` holds a curated subset of POSIX.1 scripts
with expected stdout + exit status. Every case is:

1. Run against the briSH binary, checked against expectations.
2. When `dash` is on PATH (the POSIX reference shell), run against
   `dash -c` too — a wrong expectation cannot pass.

Current corpus: **~152 cases** covering parameter expansion (`${#}`,
`#`/`%` trims, `:+`/`-`/`=`/`?`), arithmetic, quoting, redirections
(`> >| >> < << <<-`, heredoc expansions quoting), subshell isolation,
functions, `test`/`[`, pipelines, `&&`/`||`, `set`/`unset`, command
substitution, `trap EXIT`, `getopts`, alias-off-in-batch, globbing,
`read`, `printf` (`%s %b %d %x %o %5s %-5s %.3s`, format cycling), `test`
string/file/access predicates and `-t`, `&&`/`||` list short-circuiting,
`exec`, case-pattern groups, `for … do` without `in`, and
newline-separated function bodies.

### Documented deviations from dash

| Construct | briSH | dash | Why |
|---|---|---|---|
| `echo 'a\tb'` | literal backslash | XSI: tab | POSIX leaves `echo` escapes undefined; `-e` opt-in like bash |
| readonly reassign in a program | continues, `$?=1` | fatal exit | bash-compatible choice; POSIX allows either |
| not-found diagnostics | `brish: …` to stderr | `…: not found` (stderr) | message text, same exit 127 |
| expansion errors | exit 1 | exit 2 | POSIX only requires non-zero; briSH is uniform, dash uses 2 for every expansion failure |
| `test -nt` / `-ot` | full-precision mtime | whole seconds | briSH compares sub-second differences; dash truncates, so the two disagree on files written within the same second. Deliberate: the stricter answer is the accurate one. |

Growth: add cases to the `CASES` table; they must pass both `cargo test`
and `dash` on a machine with `/bin/dash`.

## Fuzzing

`fuzz/` holds libFuzzer targets (own workspace):

```sh
cargo +nightly fuzz run lexer    # or parser / expand
```

Smoke results (local, macOS):

| Target | Executions | Result |
|---|---|---|
| lexer | 804k | no panic |
| parser | 540k | no panic |
| expand | multiple runs, corpus >700 | no panic |

Zero-panic contract: the three targets must never find a crash. CI runs
a longer nightly time-boxed pass.

## Coverage

`cargo llvm-cov --workspace` (plan §9 target ≥80% lines in core):

| Metric | Value |
|---|---|
| Lines | **83.65%** |
| Functions | 77.42% |
| Regions | 81.26% |

`brish/src/main.rs` (REPL/tty paths) and `edit_mode.rs` are the low
spots (41%/43%) — interactive code, exercised manually, kept lean by
moving logic into tested modules (`highlight.rs` 93%, `completion.rs`
96%, `config.rs` 91%).

## Interactive latency

`highlight_latency_smoke` renders 2000 realistic lines in the test
suite: **~27ms total ⇒ ~13µs per keystroke render** (plan §9 budget:
<50ms per typical line). The autosuggest/completion paths share the
same lexer and are bounded by the same cost.