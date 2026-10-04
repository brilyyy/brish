//! `venv-prompt` — [`PromptSegment`](crate::PromptSegment) showing the
//! active Python virtual environment.
//!
//! Pure: reads `$VIRTUAL_ENV` (set by virtualenv/venv/hatch) and renders
//! the basename. No subprocess. 1s TTL cache via [`segments::Ttl`].

use crate::{Plugin, PromptSegment};
use crate::builtin::segments::{paint, Ttl};
use std::path::Path;

pub struct VenvPrompt {
    ttl: Ttl,
}

impl Default for VenvPrompt {
    fn default() -> Self {
        Self { ttl: Ttl::new() }
    }
}

impl VenvPrompt {
    fn render_now(&self) -> Option<String> {
        let venv = std::env::var_os("VIRTUAL_ENV")?;
        let p = Path::new(&venv);
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|s| !s.is_empty())?;
        Some(paint("35", name))
    }
}

impl PromptSegment for VenvPrompt {
    fn render(&self, _status: i32, cwd: &Path) -> Option<String> {
        self.ttl.get(cwd, || self.render_now())
    }
}

impl Plugin for VenvPrompt {
    fn name(&self) -> &str {
        "venv-prompt"
    }

    fn install(&self, reg: &mut crate::Registry) {
        reg.prompt_segments.push(Box::new(Self::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn restore(prev: Option<std::ffi::OsString>) {
        // SAFETY: restoring prior value.
        match prev {
            Some(v) => unsafe { std::env::set_var("VIRTUAL_ENV", v) },
            None => unsafe { std::env::remove_var("VIRTUAL_ENV") },
        }
    }

    #[test]
    fn no_virtual_env_yields_none() {
        let _g = lock();
        // SAFETY: test-scoped; restored below.
        let prev = std::env::var_os("VIRTUAL_ENV");
        unsafe { std::env::remove_var("VIRTUAL_ENV") };
        let p = VenvPrompt::default();
        assert_eq!(p.render(0, std::env::current_dir().unwrap().as_path()), None);
        restore(prev);
    }

    #[test]
    fn renders_basename_with_color() {
        let _g = lock();
        let tmp = tempfile::tempdir().unwrap();
        let venv = tmp.path().join("myenv");
        std::fs::create_dir_all(&venv).unwrap();
        // SAFETY: test-scoped; restored immediately.
        let prev = std::env::var_os("VIRTUAL_ENV");
        unsafe { std::env::set_var("VIRTUAL_ENV", &venv) };
        let p = VenvPrompt::default();
        let out = p.render(0, std::env::current_dir().unwrap().as_path());
        // Color enabled under NO_COLOR absent (test env): expect magenta-wrapped basename.
        assert_eq!(out, Some(paint("35", "myenv")));
        restore(prev);
    }
}