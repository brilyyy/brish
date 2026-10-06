//! Default edit-mode and menu catalog plugins.
//!
//! These wrap the hardcoded reedline integrations that used to live
//! directly in `edit_repl` so they can be replaced by user plugins.

use reedline::{ColumnarMenu, DefaultValidator, FileBackedHistory, ListMenu, MenuBuilder};

/// Plugin that installs the default Emacs edit mode.
pub struct EmacsModePlugin;

impl brish_plugin::Plugin for EmacsModePlugin {
    fn name(&self) -> &str {
        "brish-emacs"
    }

    fn install(&self, reg: &mut brish_plugin::Registry) {
        struct F;
        impl brish_plugin::EditModeFactory for F {
            fn create(&self, keybindings: reedline::Keybindings) -> Box<dyn reedline::EditMode> {
                Box::new(reedline::Emacs::new(keybindings))
            }
        }
        reg.edit_mode_factories.push(Box::new(F));
    }
}

/// Plugin that installs vi keybindings.
pub struct ViModePlugin;

impl brish_plugin::Plugin for ViModePlugin {
    fn name(&self) -> &str {
        "brish-vi"
    }

    fn install(&self, reg: &mut brish_plugin::Registry) {
        struct F;
        impl brish_plugin::EditModeFactory for F {
            fn create(&self, keybindings: reedline::Keybindings) -> Box<dyn reedline::EditMode> {
                let kb = keybindings;
                Box::new(reedline::Vi::new(kb.clone(), kb.clone(), kb))
            }
        }
        reg.edit_mode_factories.push(Box::new(F));
    }
}

/// Plugin that installs the default completion menu.
pub struct DefaultMenusPlugin;

impl brish_plugin::Plugin for DefaultMenusPlugin {
    fn name(&self) -> &str {
        "brish-menus"
    }

    fn install(&self, reg: &mut brish_plugin::Registry) {
        struct F;
        impl brish_plugin::MenuFactory for F {
            fn create(&self) -> Box<dyn reedline::Menu> {
                // Muted descriptions by default (NOTES.md 8); config
                // `[prompt] completion_description` overrides (off = plain).
                let style = crate::config::completion_desc_style()
                    .unwrap_or_else(|| nu_ansi_term::Color::DarkGray.normal());
                Box::new(
                    ColumnarMenu::default()
                        .with_name(crate::keymap::MENU_NAME)
                        .with_description_text_style(style),
                )
            }
        }
        reg.menu_factories.push(Box::new(F));
    }
}

/// Plugin that installs the history-search menu (Ctrl-R).
pub struct HistorySearchPlugin;

impl brish_plugin::Plugin for HistorySearchPlugin {
    fn name(&self) -> &str {
        "brish-history-search"
    }

    fn install(&self, reg: &mut brish_plugin::Registry) {
        struct F;
        impl brish_plugin::MenuFactory for F {
            fn create(&self) -> Box<dyn reedline::Menu> {
                Box::new(ListMenu::default().with_name(crate::keymap::HISTORY_MENU))
            }
        }
        reg.menu_factories.push(Box::new(F));
    }
}

/// Plugin that installs the default file-backed history backend.
pub struct HistoryPlugin;

impl brish_plugin::Plugin for HistoryPlugin {
    fn name(&self) -> &str {
        "brish-history"
    }

    fn install(&self, reg: &mut brish_plugin::Registry) {
        struct F;
        impl brish_plugin::HistoryFactory for F {
            fn create(&self) -> Box<dyn reedline::History> {
                match FileBackedHistory::with_file(1000, crate::config::history_path()) {
                    Ok(h) => Box::new(h),
                    Err(e) => {
                        eprintln!("brish: history unavailable: {e}");
                        Box::new(FileBackedHistory::default())
                    }
                }
            }
        }
        reg.history_factories.push(Box::new(F));
    }
}

/// Plugin that installs the default validator.
pub struct ValidatorPlugin;

impl brish_plugin::Plugin for ValidatorPlugin {
    fn name(&self) -> &str {
        "brish-validator"
    }

    fn install(&self, reg: &mut brish_plugin::Registry) {
        struct F;
        impl brish_plugin::ValidatorFactory for F {
            fn create(&self) -> Box<dyn reedline::Validator> {
                Box::new(DefaultValidator)
            }
        }
        reg.validator_factories.push(Box::new(F));
    }
}
