# WASM segments that misbehave on purpose

Two plugins whose guests are broken in the two ways most likely to matter
— a runaway loop and a runaway return value. Both are here so the limits
documented in [`docs/PLUGIN-WASM.md`](../../docs/PLUGIN-WASM.md) can be
*seen* rather than taken on faith:

| plugin | misbehavior | what the shell does |
|---|---|---|
| `fuel-burner` | `(loop $l (br $l))` — never returns | burns `fuel` operations, traps, no segment, one warning |
| `oversized` | returns 200 bytes with `max_output = 32` | rejects before reading, no segment, one warning |

Neither can take the prompt down. Install one, use the shell normally,
and watch the stderr warning while typing keeps working.

## Try it

Build briSH with the feature, then:

```sh
brish plugin add ./fuel-burner
# every prompt: brish: wasm segment `wasm-fuel-burner` (wasm-fuel-burner) trapped or ran out of fuel
```

To see it in the prompt itself, point a theme at the segments — the
`wasm-status` example's theme does:

```toml
[theme]
name = "seg"
prompt = "{arrow} {cwd} |{segments}|"
```

## The `.wat` files are the source of truth

Each directory holds a hand-written `.wat` and the compiled `.wasm`.
They are byte-for-byte the same modules the crate's tests build inline:

- `fuel-burner.wat` ↔ `out_of_fuel_yields_no_segment`
- `oversized.wat` ↔ `oversized_output_is_rejected`

so a change to the host's enforcement belongs in
`crates/brish-plugin-wasm/src/lib.rs` **and** here. To recompile after
editing a `.wat`, any WAT assembler works:

```sh
wat2wasm fuel-burner/fuel-burner.wat -o fuel-burner/fuel-burner.wasm
```

The committed binaries mean no assembler is needed to read or run these
examples — only to change them.