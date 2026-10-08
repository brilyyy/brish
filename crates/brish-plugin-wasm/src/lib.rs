//! WASM prompt segments for briSH.
//!
//! A `[wasm]` store plugin runs a WebAssembly module in the `wasmi`
//! interpreter and uses its return value as prompt segment text. The
//! guest gets **no** host imports at all: no filesystem, no network, no
//! process spawn, no clock. It sees only the bytes the host writes into
//! its linear memory, and the only way out is the string it returns.
//!
//! # ABI
//!
//! The module must export two functions. Everything else is ignored.
//!
//! ```wat
//! ;; alloc(ptr: i32, len: i32) -> i32
//! ;;   Host writes `len` bytes at `ptr`; return an offset with room for
//! ;;   them inside the guest's linear memory (a bump allocator is fine).
//! (func (export "alloc") (param i32 i32) (result i32) …)
//!
//! ;; render(ptr: i32, len: i32, status: i32, cwd_ptr: i32, cwd_len: i32) -> i64
//! ;;   Returns (out_ptr << 32) | out_len. out_len 0 means "no segment".
//! (func (export "render") (param i32 i32 i32 i32 i32) (result i64) …)
//! ```
//!
//! `ptr/len` is the status code and cwd the host serialized for this
//! prompt. A guest that returns a length beyond `max_output`, a pointer
//! outside its memory, or runs out of fuel yields no segment and one
//! warning — a bad plugin must never take the prompt down.
//!
//! # Ceilings
//!
//! - One mutex per segment: the seam traits are `&self`, so the
//!   `Store` needs interior mutability. One lock per prompt render.
//! - Fuel is the deadline. There is no process to kill, so the fuel
//!   budget is the only bound on guest work.
//! - The module is instantiated once per session, never per render.
//! - `PromptSegment` only. Hooks, completion and keymap seams are steps
//!   3-5 of `docs/PLUGIN-WASM.md`; the reedline factory seams return
//!   in-process object graphs and can never cross this boundary.

use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use brish_builtin::store::WasmDecl;
use brish_plugin_api::PromptSegment;
use wasmi::{Engine, Extern, Instance, Linker, Memory, Module, Store, TypedFunc};

/// `alloc(ptr: i32, len: i32) -> i32`
type AllocFn = TypedFunc<(i32, i32), i32>;
/// `render(ptr, len, status, cwd_ptr, cwd_len) -> i64`
type RenderFn = TypedFunc<(i32, i32, i32, i32, i32), i64>;

/// One instantiated module behind its store. The exports are resolved
/// once at load: a render that goes through `get_export` on every prompt
/// pays for a string lookup per keystroke-path call for no reason.
struct Loaded {
    store: Store<()>,
    memory: Memory,
    alloc: AllocFn,
    render: RenderFn,
}

/// Load failures, all non-fatal to the shell: the plugin is skipped and
/// the caller warns once.
#[derive(Debug)]
pub enum Error {
    Read(std::io::Error),
    InvalidModule(wasmi::Error),
    /// The module parsed but does not export the ABI we require.
    BadAbi(&'static str),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Read(e) => write!(f, "{e}"),
            Error::InvalidModule(e) => write!(f, "invalid wasm module: {e}"),
            Error::BadAbi(what) => write!(f, "missing or malformed `{what}` export"),
        }
    }
}

impl std::error::Error for Error {}

/// Load a `[wasm]` manifest into `registry` as a prompt segment.
/// Returns the warning to print, or `None` on success. A bad module is
/// never fatal: the shell starts without the segment.
pub fn install(
    registry: &mut brish_plugin_api::Registry,
    plugin: &str,
    dir: &Path,
    decl: &WasmDecl,
) -> Option<String> {
    match WasmSegment::load(plugin, plugin, dir, decl) {
        Ok(seg) => {
            registry.prompt_segments.push(Box::new(seg));
            None
        }
        Err(e) => Some(format!("wasm segment `{plugin}`: {e}")),
    }
}

/// A WASM prompt segment. `render` runs on every prompt, so the module
/// is instantiated once here and reused behind the mutex.
pub struct WasmSegment {
    plugin: String,
    name: String,
    fuel: u64,
    max_output: usize,
    inner: Mutex<Loaded>,
    warned: AtomicBool,
}

impl WasmSegment {
    /// Read and instantiate `decl.path` (relative to the plugin `dir`).
    /// `name` labels the segment in warnings; a `[wasm]` manifest has no
    /// name of its own because the segment *is* the plugin.
    pub fn load(plugin: &str, name: &str, dir: &Path, decl: &WasmDecl) -> Result<Self, Error> {
        let wasm = std::fs::read(dir.join(&decl.path)).map_err(Error::Read)?;
        // Fuel metering is the deadline: without it a guest looping
        // forever would hang the prompt.
        let mut config = wasmi::Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, &wasm[..]).map_err(Error::InvalidModule)?;

        // No host functions are defined, so any import the guest asks
        // for fails here — that is the default-deny boundary.
        let mut store = Store::new(&engine, ());
        let instance = Linker::new(&engine)
            .instantiate_and_start(&mut store, &module)
            .map_err(Error::InvalidModule)?;

        let (memory, alloc, render) = check_abi(&store, &instance)?;

        Ok(Self {
            plugin: plugin.to_string(),
            name: name.to_string(),
            fuel: decl.fuel,
            max_output: decl.max_output,
            inner: Mutex::new(Loaded {
                store,
                memory,
                alloc,
                render,
            }),
            warned: AtomicBool::new(false),
        })
    }

    /// Print a plugin problem once per session. A guest that traps on
    /// every prompt would otherwise scroll a line per render.
    fn warn_once(&self, msg: String) {
        if !self.warned.swap(true, Ordering::Relaxed) {
            eprintln!("brish: {msg}");
        }
    }
}

/// Resolve the ABI: `memory`, `alloc`, `render`. All three are required
/// — without `memory` there is nowhere to write the inputs, and a wrong
/// signature would fail later inside the prompt.
fn check_abi(store: &Store<()>, instance: &Instance) -> Result<(Memory, AllocFn, RenderFn), Error> {
    let Some(Extern::Memory(memory)) = instance.get_export(store, "memory") else {
        return Err(Error::BadAbi("memory"));
    };
    let Some(Extern::Func(f)) = instance.get_export(store, "alloc") else {
        return Err(Error::BadAbi("alloc"));
    };
    let alloc = f
        .typed::<(i32, i32), i32>(store)
        .map_err(|_| Error::BadAbi("alloc"))?;
    let Some(Extern::Func(f)) = instance.get_export(store, "render") else {
        return Err(Error::BadAbi("render"));
    };
    let render = f
        .typed::<(i32, i32, i32, i32, i32), i64>(store)
        .map_err(|_| Error::BadAbi("render"))?;
    Ok((memory, alloc, render))
}

impl PromptSegment for WasmSegment {
    fn render(&self, status: i32, cwd: &Path) -> Option<String> {
        let cwd = cwd.to_string_lossy();
        let mut guard = self.inner.lock().ok()?;
        let Loaded {
            store,
            memory,
            alloc,
            render,
        } = &mut *guard;

        // Fuel is per-call, not per-session: reset before each render.
        if store.set_fuel(self.fuel).is_err() {
            self.warn_once(format!(
                "segment `{}` ({}) could not set fuel",
                self.name, self.plugin
            ));
            return None;
        }

        // Ask the guest where to write each input. Status first, then
        // cwd — two independent allocations, so the guest's allocator
        // does not need to understand a combined request.
        let status_bytes = status.to_string();
        let status_len = i32::try_from(status_bytes.len()).ok()?;
        let cwd_len = i32::try_from(cwd.len()).ok()?;
        let ptr = alloc_input(alloc, &mut *store, status_len)?;
        write(memory, &mut *store, ptr, status_bytes.as_bytes())?;
        let cwd_ptr = alloc_input(alloc, &mut *store, cwd_len)?;
        write(memory, &mut *store, cwd_ptr, cwd.as_bytes())?;

        // Out of fuel, a trap, a bad pointer: all the same to the
        // prompt. Warn once so a runaway guest stays visible without
        // scrolling a line on every render.
        let packed = match render.call(&mut *store, (ptr, status_len, status, cwd_ptr, cwd_len)) {
            Ok(packed) => packed,
            Err(_) => {
                self.warn_once(format!(
                    "segment `{}` ({}) trapped or ran out of fuel",
                    self.name, self.plugin
                ));
                return None;
            }
        };

        let out_len = usize::try_from(packed & 0xffff_ffff).ok()?;
        if out_len == 0 {
            return None;
        }
        if out_len > self.max_output {
            self.warn_once(format!(
                "segment `{}` ({}) returned {out_len} bytes, over the {}-byte cap",
                self.name, self.plugin, self.max_output
            ));
            return None;
        }
        let data = memory.data(store);
        let start = usize::try_from((packed >> 32) as u32).ok()?;
        let end = start.checked_add(out_len)?;
        let bytes = data.get(start..end)?;
        // A segment is prompt text, not a control channel: never let a
        // guest inject escapes or newlines into the prompt line.
        Some(sanitize(&String::from_utf8_lossy(bytes)))
    }
}

/// Ask the guest for `len` bytes of room. The pointer it returns is
/// checked in [`write`], so a guest that hands back nonsense costs
/// nothing but a missing segment.
fn alloc_input(alloc: &AllocFn, store: &mut Store<()>, len: i32) -> Option<i32> {
    alloc.call(store, (0, len)).ok()
}

/// Copy `bytes` into guest memory at `ptr`, bounds-checked by wasmi.
fn write(mem: &Memory, store: &mut Store<()>, ptr: i32, bytes: &[u8]) -> Option<()> {
    let start = usize::try_from(ptr).ok()?;
    mem.write(&mut *store, start, bytes).ok()?;
    Some(())
}

/// Drop control characters and escape sequences a guest could otherwise
/// use to redraw the prompt or inject newlines into it.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() && *c != '\u{1b}')
        .take(4096)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A guest that returns a fixed string, written in WAT so the test
    /// needs no wasm toolchain.
    const HELLO: &str = r#"
(module
  (memory (export "memory") 1)
  (global $bump (mut i32) (i32.const 1024))
  (data (i32.const 2048) "wasm-ok")
  (func (export "alloc") (param $ptr i32) (param $len i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (local.get $p) (local.get $len)))
    (local.get $p))
  (func (export "render") (param i32 i32 i32 i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 2048) (i64.const 32)) (i64.const 7))))
"#;

    fn decl(fuel: u64, max_output: usize) -> WasmDecl {
        WasmDecl {
            path: "p.wasm".into(),
            fuel,
            max_output,
        }
    }

    fn write_module(dir: &Path, wat: &str) {
        let bytes = wat::parse_str(wat).expect("valid wat");
        std::fs::write(dir.join("p.wasm"), bytes).expect("write module");
    }

    #[test]
    fn renders_the_guest_string() {
        let d = tempfile::tempdir().expect("tmpdir");
        write_module(d.path(), HELLO);
        let seg =
            WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096)).expect("load");
        assert_eq!(
            seg.render(0, Path::new("/tmp")),
            Some("wasm-ok".to_string())
        );
    }

    #[test]
    fn zero_length_result_is_no_segment() {
        let d = tempfile::tempdir().expect("tmpdir");
        write_module(
            d.path(),
            r#"
(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32 i32) (result i32) (i32.const 1024))
  (func (export "render") (param i32 i32 i32 i32 i32) (result i64) (i64.const 0)))
"#,
        );
        let seg =
            WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096)).expect("load");
        assert_eq!(seg.render(0, Path::new("/tmp")), None);
    }

    #[test]
    fn missing_export_is_a_load_error() {
        let d = tempfile::tempdir().expect("tmpdir");
        write_module(d.path(), r#"(module (memory (export "memory") 1))"#);
        let err = WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096));
        assert!(matches!(err, Err(Error::BadAbi(_))));
    }

    #[test]
    fn wrong_signature_is_a_load_error() {
        let d = tempfile::tempdir().expect("tmpdir");
        // render takes the wrong arity — must not be accepted.
        write_module(
            d.path(),
            r#"
(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32 i32) (result i32) (i32.const 1024))
  (func (export "render") (param i32) (result i64) (i64.const 0)))
"#,
        );
        let err = WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096));
        assert!(matches!(err, Err(Error::BadAbi("render"))));
    }

    #[test]
    fn missing_memory_export_is_a_load_error() {
        // No memory at all: there would be nowhere to write the inputs.
        let d = tempfile::tempdir().expect("tmpdir");
        write_module(
            d.path(),
            r#"
(module
  (func (export "alloc") (param i32 i32) (result i32) (i32.const 1024))
  (func (export "render") (param i32 i32 i32 i32 i32) (result i64) (i64.const 0)))
"#,
        );
        let err = WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096));
        assert!(matches!(err, Err(Error::BadAbi("memory"))));
    }

    #[test]
    fn malformed_module_is_an_error_not_a_panic() {
        let d = tempfile::tempdir().expect("tmpdir");
        std::fs::write(d.path().join("p.wasm"), b"not wasm at all").expect("write");
        let err = WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096));
        assert!(matches!(err, Err(Error::InvalidModule(_))));
    }

    #[test]
    fn missing_file_is_an_error() {
        let d = tempfile::tempdir().expect("tmpdir");
        let err = WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096));
        assert!(matches!(err, Err(Error::Read(_))));
    }

    #[test]
    fn no_imports_are_satisfiable() {
        // Default-deny: a guest asking for a host function must not link.
        let d = tempfile::tempdir().expect("tmpdir");
        write_module(
            d.path(),
            r#"
(module
  (import "env" "read_file" (func $r (param i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "alloc") (param i32 i32) (result i32) (i32.const 1024))
  (func (export "render") (param i32 i32 i32 i32 i32) (result i64) (i64.const 0)))
"#,
        );
        let err = WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096));
        assert!(matches!(err, Err(Error::InvalidModule(_))));
    }

    #[test]
    fn out_of_fuel_yields_no_segment() {
        let d = tempfile::tempdir().expect("tmpdir");
        // An infinite loop: fuel is the only thing that stops it.
        write_module(
            d.path(),
            r#"
(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32 i32) (result i32) (i32.const 1024))
  (func (export "render") (param i32 i32 i32 i32 i32) (result i64)
    (loop $l (br $l))
    (i64.const 0)))
"#,
        );
        let seg = WasmSegment::load("demo", "demo", d.path(), &decl(50_000, 4096)).expect("load");
        assert_eq!(seg.render(0, Path::new("/tmp")), None);
    }

    #[test]
    fn oversized_output_is_rejected() {
        let d = tempfile::tempdir().expect("tmpdir");
        // Claims 100 bytes; the cap is 8.
        write_module(
            d.path(),
            r#"
(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32 i32) (result i32) (i32.const 1024))
  (func (export "render") (param i32 i32 i32 i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 2048) (i64.const 32)) (i64.const 100))))
"#,
        );
        let seg = WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 8)).expect("load");
        assert_eq!(seg.render(0, Path::new("/tmp")), None);
    }

    #[test]
    fn out_of_range_pointer_is_rejected() {
        let d = tempfile::tempdir().expect("tmpdir");
        // 1 GiB into a 64 KiB memory: must not panic.
        write_module(
            d.path(),
            r#"
(module
  (memory (export "memory") 1)
  (func (export "alloc") (param i32 i32) (result i32) (i32.const 1024))
  (func (export "render") (param i32 i32 i32 i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 1073741824) (i64.const 32)) (i64.const 4))))
"#,
        );
        let seg =
            WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096)).expect("load");
        assert_eq!(seg.render(0, Path::new("/tmp")), None);
    }

    #[test]
    fn control_characters_are_stripped_from_output() {
        let d = tempfile::tempdir().expect("tmpdir");
        // "a\nb" plus an ESC — the guest cannot inject newlines or
        // escapes into the prompt line.
        write_module(
            d.path(),
            r#"
(module
  (memory (export "memory") 1)
  ;; "a", newline, "b", ESC, "[31m" — 9 bytes
  (data (i32.const 2048) "a\nb\1b[31m")
  (func (export "alloc") (param $ptr i32) (param $len i32) (result i32) (i32.const 1024))
  (func (export "render") (param i32 i32 i32 i32 i32) (result i64)
    (i64.or (i64.shl (i64.const 2048) (i64.const 32)) (i64.const 9))))
"#,
        );
        let seg =
            WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096)).expect("load");
        // The newline and the ESC are gone; the rest survives verbatim.
        assert_eq!(seg.render(0, Path::new("/tmp")), Some("ab[31m".into()));
    }

    #[test]
    fn guest_sees_status_and_cwd_bytes() {
        // The guest that echoes back what the host wrote, proving the
        // input path (alloc + memory write) actually lands.
        let d = tempfile::tempdir().expect("tmpdir");
        write_module(
            d.path(),
            r#"
(module
  (memory (export "memory") 1)
  (global $bump (mut i32) (i32.const 1024))
  (func (export "alloc") (param $ptr i32) (param $len i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (local.get $p) (local.get $len)))
    (local.get $p))
  ;; echo back exactly what the host wrote for the status code
  (func (export "render") (param $ptr i32) (param $len i32) (param $status i32)
        (param $cwd_ptr i32) (param $cwd_len i32) (result i64)
    (i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
            (i64.extend_i32_u (local.get $len)))))
"#,
        );
        let seg =
            WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096)).expect("load");
        assert_eq!(seg.render(42, Path::new("/tmp")), Some("42".into()));
    }

    #[test]
    fn guest_receives_the_real_cwd_bytes() {
        // Echoes the cwd back: proves the cwd input path lands where
        // the guest can read it.
        let d = tempfile::tempdir().expect("tmpdir");
        write_module(
            d.path(),
            r#"
(module
  (memory (export "memory") 1)
  (global $bump (mut i32) (i32.const 1024))
  (func (export "alloc") (param $ptr i32) (param $len i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (local.get $p) (local.get $len)))
    (local.get $p))
  (func (export "render") (param $ptr i32) (param $len i32) (param $status i32)
        (param $cwd_ptr i32) (param $cwd_len i32) (result i64)
    (i64.or (i64.shl (i64.extend_i32_u (local.get $cwd_ptr)) (i64.const 32))
            (i64.extend_i32_u (local.get $cwd_len)))))
"#,
        );
        let seg =
            WasmSegment::load("demo", "demo", d.path(), &decl(1_000_000, 4096)).expect("load");
        assert_eq!(
            seg.render(0, Path::new("/a/deep/path")),
            Some("/a/deep/path".into())
        );
    }
}
