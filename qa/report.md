# briSH QA report — differential sweep vs dash/bash

Date: 2026-10-08 · binary: `briSH 0.1.0` (release, inside `brish-qa:latest`)
· baseline: `b6c0da9`

## Method

Two harnesses, both running the real binary in `debian:bookworm-slim`
as an **unprivileged** user (uid 0 makes `-r`/`-w`/`-x` always true and
would hide the exact bug class under test).

| Layer | Tool | Question it answers |
|---|---|---|
| 1a | `qa/sweep-parse.sh` | Can briSH *parse* the shell scripts a real Debian ships? |
| 1b | `qa/diff-snippets.sh` | On hermetic snippets, does it produce the same **stdout and exit status** as `dash`? |

Design notes that mattered:

- **Parse-only gate via `set -n`.** briSH parses the whole file before
  executing, so `set -n` + a status check gives a safe sweep — running
  `apt` config hooks or `update-*` scripts for real would reconfigure the
  container. Parse failures are also gated on `dash -n` first, so
  truncated quotes or deliberately-broken scripts are skipped instead of
  reported.
- **stdout and status compared separately.** All four bugs found in this
  round (and all four from the previous round) were status-only or
  silent. A combined comparison hides exactly the class we care about.
- **Status travels through a file**, never through stdout — a snippet
  that prints newlines would otherwise corrupt the transport.
- **`PATH` includes `/usr/local/bin`.** An omitted oracle reads as
  "exit 127 = bug" and manufactures false findings.

## Layer 1a — parse sweep: 55 sh/bash scripts, 11 failures → 0

Baseline was 11/55 failing. Every one was a real bug in briSH, and every
one broke scripts Debian actually ships.

### Bug 1 — `(` opens a *group* in a case pattern, it is not a literal

`set -n` rejected 11 scripts. First bisect pointed at `zgrep` and
`which.debianutils`, both of which write `case $x in (*[!:]:) …`.

```
$ brish -c 'case ab in (ab) printf m ;; esac'   → parse error
$ dash  'case ab in (ab) printf m ;; esac'       → m
```

POSIX 2.6.4 `case_pattern` is `'(' pattern` — the parens are grouping
delimiters and are **not** part of the pattern, so `(ab)` matches the
plain string `ab`. briSH treated the `(` as an unexpected token.

A first attempt made `(` *literal*, which fixed the parse but broke the
match (`(ab)` matched nothing). Corrected to consume the `(` as a
grouping delimiter.

Affected in the image: `zgrep`, `which.debianutils`.

### Bug 2 — `for i do … done` (omitted `in` list) rejected

`zforce`, `znew`:

```sh
for i do            # == for i in "$@"; do
```

POSIX allows the `in` list and its separator to be omitted. The engine
already modelled `words: None` as "iterate `$@`" — only the parser
rejected the syntax.

### Bug 3 — function definition with `{` on the next line

5 scripts: `hwclock.sh`, `dpkg-realpath`, `ldd`, `savelog`,
`dpkg-maintscript-helper`.

```sh
show_version()
{
  …
}
```

POSIX `no_newlines` formally disallows this, but it is the dominant
house style in Debian's own scripts and both oracles accept it. Now
skipped rather than rejected.

### Not bugs (correct rejections, verified)

`/usr/sbin/e2scrub_all` — `dash -n` fails on it too, so it is skipped by
the harness, not counted.

## Layer 1b — 201 hermetic snippets, 3 findings

Corpus targets the classes that have actually bitten us: `test`/`[`
truth tables, `&&`/`||` precedence, subshell isolation, quoting, IFS,
redirections, loops, functions, traps, builtins — plus the constructs
fixed above.

### Bug 4 — `exec` was not a builtin at all

```
$ brish -c 'exec 3>&1; echo via-fd3 >&3'
brish: exec: command not found
```

`exec` is a POSIX special builtin; it was missing from `BuiltIn` and fell
through to the PATH search, so the standard fd-juggling idiom died on
the first statement. Implemented: bare `exec` applies its redirections
permanently to the current shell; `exec cmd` replaces the shell (fork +
exec + exit with the child's status, so EXIT traps still run).

Verified against dash: `exec true/false` propagate status, `exec cmd`
suppresses later lines, redirect-only works.

### Two remaining STATUS deltas — documented deviations, not bugs

| Snippet | dash | bash | briSH |
|---|---|---|---|
| `x=abc; echo "${x:1}"` | 2 | 0 | 1 |
| `echo $((2**3))` | 2 | 1 | 1 |

Both are bash-isms that briSH correctly rejects (it is not required to
implement them). The delta is only the **status code of the rejection**.
For `${x:1}` and `2**3` bash *accepts* them, so bash=0 there.

The general rule found: for expansion errors briSH exits 1, dash exits 2,
and bash varies (127 for `${x:?}`, 1 for arithmetic). POSIX only
requires "non-zero", so briSH's uniform 1 is defensible; dash's 2 is its
own convention. Recorded in `docs/POSIX.md` rather than changed.

## Coverage gap this exposes

The previous corpus had **zero** `-t`, `-r`, `-w`, `-x` and zero
mixed-operator list cases, which is precisely why those four bugs
survived. Two of this round's findings (`for i do`, `name()\n{`) were
likewise only reachable from real scripts. The lesson is the same as
last round and the harness now encodes it: curated cases only test what
someone already thought of.

## Reproducing

```sh
docker build -f qa/Dockerfile -t brish-qa .
docker run --rm -v "$PWD/qa:/qa:ro" brish-qa sh /qa/sweep-parse.sh /tmp/p
docker run --rm -v "$PWD/qa:/qa:ro" -v /tmp/out:/out \
  brish-qa sh /qa/diff-snippets.sh /qa/snippets.txt /out
```

## Known harness limitations

- `snippets.txt` is one snippet per line, so it cannot express
  multi-line constructs; those live in `crates/brish/tests/posix.rs`
  and the parse sweep instead. (An earlier version tried and produced a
  false finding about the harness itself.)
- Whole-script stdout is never compared — real scripts print paths,
  dates and pids. Only exit status is comparable for them.
- The image carries ~275 executables; a host with a full `/usr/bin`
  (~930) would widen layer 1a.