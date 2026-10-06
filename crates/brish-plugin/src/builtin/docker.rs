//! `docker-prompt` — [`PromptSegment`](crate::PromptSegment) showing
//! the active Docker context.
//!
//! Pure: reads `$DOCKER_CONTEXT` (set by `docker context use`), no
//! subprocess. 1s TTL cache via [`segments::Ttl`]. Defaults to `default`
//! implicitly (nothing printed) when the var is unset.

use crate::builtin::segments::{Ttl, env_first, paint};
use crate::{Plugin, PromptSegment};
use std::path::Path;

pub struct DockerPrompt {
    ttl: Ttl,
}

impl Default for DockerPrompt {
    fn default() -> Self {
        Self { ttl: Ttl::new() }
    }
}

impl DockerPrompt {
    fn render_now(&self) -> Option<String> {
        let ctx = env_first(&["DOCKER_CONTEXT"])?;
        Some(paint("38;5;121", format!("🐳 {ctx}")))
    }
}

impl PromptSegment for DockerPrompt {
    fn render(&self, _status: i32, cwd: &Path) -> Option<String> {
        self.ttl.get(cwd, || self.render_now())
    }
}

impl Plugin for DockerPrompt {
    fn name(&self) -> &str {
        "docker-prompt"
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
            Some(v) => unsafe { std::env::set_var("DOCKER_CONTEXT", v) },
            None => unsafe { std::env::remove_var("DOCKER_CONTEXT") },
        }
    }

    #[test]
    fn no_docker_context_yields_none() {
        let _g = lock();
        // SAFETY: test-scoped; restored below.
        let prev = std::env::var_os("DOCKER_CONTEXT");
        unsafe { std::env::remove_var("DOCKER_CONTEXT") };
        let p = DockerPrompt::default();
        assert_eq!(
            p.render(0, std::env::current_dir().unwrap().as_path()),
            None
        );
        restore(prev);
    }

    #[test]
    fn renders_context_name() {
        let _g = lock();
        // SAFETY: test-scoped; restored below.
        let prev = std::env::var_os("DOCKER_CONTEXT");
        unsafe { std::env::set_var("DOCKER_CONTEXT", "production") };
        let p = DockerPrompt::default();
        let out = p.render(0, std::env::current_dir().unwrap().as_path());
        assert_eq!(out, Some(paint("38;5;121", "🐳 production")));
        restore(prev);
    }
}
