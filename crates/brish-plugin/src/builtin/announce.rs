//! `announce-cd` — example [`ChdirHook`](crate::ChdirHook) plugin:
//! prints `old -> new` after every successful `cd`.

use crate::{ChdirHook, Plugin};
use std::path::Path;

pub struct AnnounceCd;

impl ChdirHook for AnnounceCd {
    fn on_cd(&self, old: &Path, new: &Path) {
        println!("{} -> {}", old.display(), new.display());
    }
}

impl Plugin for AnnounceCd {
    fn name(&self) -> &str {
        "announce-cd"
    }

    fn install(&self, reg: &mut crate::Registry) {
        reg.on_chdir.push(Box::new(AnnounceCd));
    }
}
