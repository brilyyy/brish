//! `aws-prompt` — [`PromptSegment`](brish_plugin::PromptSegment) showing the
//! active AWS profile/region.
//!
//! Pure: reads `$AWS_PROFILE` / `$AWS_DEFAULT_PROFILE` and `$AWS_REGION` /
//! `$AWS_DEFAULT_REGION`, no subprocess. 1s TTL cache via [`segments::Ttl`].

use crate::segments::{Ttl, env_first, paint};
use brish_plugin::{Plugin, PromptSegment};
use std::path::Path;

pub struct AwsPrompt {
    ttl: Ttl,
}

impl Default for AwsPrompt {
    fn default() -> Self {
        Self { ttl: Ttl::new() }
    }
}

impl AwsPrompt {
    fn render_now(&self) -> Option<String> {
        let profile = env_first(&["AWS_PROFILE", "AWS_DEFAULT_PROFILE"]);
        let region = env_first(&["AWS_REGION", "AWS_DEFAULT_REGION"]);
        let s = match (profile, region) {
            (Some(p), Some(r)) => format!("aws:{p}/{r}"),
            (Some(p), None) => format!("aws:{p}"),
            (None, Some(r)) => format!("aws:⋄/{r}"),
            (None, None) => return None,
        };
        Some(paint("38;5;208", s))
    }
}

impl PromptSegment for AwsPrompt {
    fn render(&self, _status: i32, cwd: &Path) -> Option<String> {
        self.ttl.get(cwd, || self.render_now())
    }
}

impl Plugin for AwsPrompt {
    fn name(&self) -> &str {
        "brish-aws"
    }

    fn install(&self, reg: &mut brish_plugin::Registry) {
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

    fn clear_env() -> Vec<(String, Option<std::ffi::OsString>)> {
        // SAFETY: test-scoped; caller restores via restore_env.
        let names = [
            "AWS_PROFILE",
            "AWS_DEFAULT_PROFILE",
            "AWS_REGION",
            "AWS_DEFAULT_REGION",
        ];
        let saved: Vec<(String, Option<std::ffi::OsString>)> = names
            .iter()
            .map(|n| (n.to_string(), std::env::var_os(n)))
            .collect();
        for (n, _) in &saved {
            unsafe { std::env::remove_var(n) };
        }
        saved
    }

    fn restore_env(saved: Vec<(String, Option<std::ffi::OsString>)>) {
        // SAFETY: restoring prior values.
        for (n, v) in &saved {
            match v {
                Some(v) => unsafe { std::env::set_var(n, v) },
                None => unsafe { std::env::remove_var(n) },
            }
        }
    }

    #[test]
    fn no_aws_env_yields_none() {
        let _g = lock();
        let saved = clear_env();
        let p = AwsPrompt::default();
        assert_eq!(
            p.render(0, std::env::current_dir().unwrap().as_path()),
            None
        );
        restore_env(saved);
    }

    #[test]
    fn renders_profile_and_region() {
        let _g = lock();
        let saved = clear_env();
        // SAFETY: test-scoped; restored below.
        unsafe {
            std::env::set_var("AWS_PROFILE", "prod");
            std::env::set_var("AWS_REGION", "us-east-1");
        }
        let p = AwsPrompt::default();
        let out = p.render(0, std::env::current_dir().unwrap().as_path());
        assert_eq!(out, Some(paint("38;5;208", "aws:prod/us-east-1")));
        restore_env(saved);
    }
}
