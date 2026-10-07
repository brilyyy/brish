//! `announce-cd` — example [`ChdirHook`](brish_plugin_api::ChdirHook) plugin:
//! prints `old -> new` after every successful `cd`.

use brish_plugin_api::{ChdirHook, Plugin};
use std::path::Path;

pub struct AnnounceCd;

impl ChdirHook for AnnounceCd {
    fn on_cd(&self, old: &Path, new: &Path) {
        println!("{} -> {}", old.display(), new.display());
    }
}

impl Plugin for AnnounceCd {
    fn name(&self) -> &str {
        "brish-announce-cd"
    }

    fn install(&self, reg: &mut brish_plugin_api::Registry) {
        reg.on_chdir.push(Box::new(AnnounceCd));
    }
}
