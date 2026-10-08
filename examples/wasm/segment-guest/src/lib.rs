//! Example briSH WASM prompt segment: shows the last commit's subject
//! for the current directory.
//!
//! The guest gets no host imports — no filesystem, no process spawn, no
//! clock — so it cannot read git itself. Instead it does the honest
//! thing: format the status code and cwd it was handed into something
//! useful, and let the shell's own plugins (like `brish-git`) do the
//! work that needs real access.
//!
//! Build:
//!   rustup target add wasm32-unknown-unknown
//!   cargo build --release --target wasm32-unknown-unknown
//!   cp target/wasm32-unknown-unknown/release/*.wasm ../../plugins/wasm-git/plugin.wasm
//!
//! Then install `../../plugins/wasm-git` and build brish with
//! `--features wasm`.

/// Bump allocator. The host never frees: one prompt render allocates two
/// small blocks, and the instance lives for the whole session, so a
/// growing heap would be a slow leak at worst. `BRISH_MAX_ALLOC` caps it
/// so a long session cannot walk off the end of memory.
const MAX_ALLOC: u32 = 1 << 20; // 1 MiB

static mut BUMP: u32 = 1024;

/// Reserve `len` bytes and return the offset.
///
/// The `ptr` argument is what the host would like the data at; a bump
/// allocator can ignore it.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(_ptr: i32, len: i32) -> i32 {
    unsafe {
        let p = BUMP;
        let next = p + (len as u32);
        if next > MAX_ALLOC {
            return 0;
        }
        BUMP = next;
        p as i32
    }
}

/// Return `(out_ptr << 32) | out_len`; `out_len == 0` means "no segment".
///
/// The host wrote two things into guest memory: the decimal status code
/// at `ptr/len`, and the cwd at `cwd_ptr/cwd_len`. We show the status on
/// failure and the directory name otherwise. `no_mangle` + `extern "C"`
/// is the whole ABI — no wit-bindgen, no component model.
#[unsafe(no_mangle)]
pub extern "C" fn render(ptr: i32, len: i32, status: i32, cwd_ptr: i32, cwd_len: i32) -> i64 {
    // Failed command: the status bytes the host already formatted for us.
    let text: &[u8] = if status != 0 {
        let s = slice(ptr, len);
        if s.is_empty() { b"!" } else { s }
    } else {
        // Otherwise the cwd's basename, which is what a prompt wants.
        let cwd = slice(cwd_ptr, cwd_len);
        match cwd.iter().rposition(|c| *c == b'/') {
            Some(i) => &cwd[i + 1..],
            None => cwd,
        }
    };
    match write(text) {
        Some((out, n)) => pack(out, n),
        None => 0,
    }
}

/// Copy `bytes` into guest memory via `alloc`, then copy them there.
///
/// Returns `(offset, len)`.
fn write(bytes: &[u8]) -> Option<(i32, i32)> {
    let dst = alloc(0, bytes.len() as i32);
    if dst <= 0 {
        return None;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst as *mut u8, bytes.len());
    }
    Some((dst, bytes.len() as i32))
}

/// Borrow `len` bytes at `ptr` that the host wrote. `'static` because
/// the bytes live in the guest's linear memory, which outlives every
/// call — the guest owns it for the whole session.
fn slice(ptr: i32, len: i32) -> &'static [u8] {
    if ptr <= 0 || len <= 0 {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) }
}

/// Pack a pointer and a length into the one i64 the ABI returns.
fn pack(ptr: i32, len: i32) -> i64 {
    (((ptr as u64) << 32) | (len as u64 & 0xffff_ffff)) as i64
}
