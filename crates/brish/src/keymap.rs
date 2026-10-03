//! Keymap provider merge (plan P3): plugins contribute stringly
//! `(key, event)` pairs; this module parses them into reedline
//! bindings. Unknown keys/events warn and skip; a key bound by several
//! providers warns once and the last binding wins.

use brish_plugin::KeymapProvider;
use reedline::{EditCommand, KeyCode, KeyModifiers, Keybindings, ReedlineEvent};
use std::collections::HashMap;

/// Menu the shell registers for completion (Tab opens it).
pub const MENU_NAME: &str = "completion_menu";
pub const HISTORY_MENU: &str = "history_menu";

/// `tab`, `ctrl-x`, `alt-1`, `shift-tab`, `up`, single chars, …
pub fn parse_key(desc: &str) -> Option<(KeyCode, KeyModifiers)> {
    let d = desc.to_ascii_lowercase();
    Some(match d.as_str() {
        "tab" => (KeyCode::Tab, KeyModifiers::NONE),
        "backtab" | "shift-tab" => (KeyCode::BackTab, KeyModifiers::SHIFT),
        "enter" => (KeyCode::Enter, KeyModifiers::NONE),
        "escape" => (KeyCode::Esc, KeyModifiers::NONE),
        "up" => (KeyCode::Up, KeyModifiers::NONE),
        "down" => (KeyCode::Down, KeyModifiers::NONE),
        "left" => (KeyCode::Left, KeyModifiers::NONE),
        "right" => (KeyCode::Right, KeyModifiers::NONE),
        "backspace" => (KeyCode::Backspace, KeyModifiers::NONE),
        "delete" => (KeyCode::Delete, KeyModifiers::NONE),
        "home" => (KeyCode::Home, KeyModifiers::NONE),
        "end" => (KeyCode::End, KeyModifiers::NONE),
        "pageup" => (KeyCode::PageUp, KeyModifiers::NONE),
        "pagedown" => (KeyCode::PageDown, KeyModifiers::NONE),
        s => {
            let (mods, rest) = if let Some(r) = s.strip_prefix("ctrl-") {
                (KeyModifiers::CONTROL, r)
            } else if let Some(r) = s.strip_prefix("alt-") {
                (KeyModifiers::ALT, r)
            } else if let Some(r) = s.strip_prefix("shift-") {
                (KeyModifiers::SHIFT, r)
            } else {
                (KeyModifiers::NONE, s)
            };
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => (KeyCode::Char(c), mods),
                _ => return None,
            }
        }
    })
}

/// Event names plugins may use. Keep small and documented — this is
/// the plugin crate's engine-free vocabulary.
pub fn parse_event(name: &str) -> Option<ReedlineEvent> {
    Some(match name {
        "menu" => ReedlineEvent::Menu(MENU_NAME.to_string()),
        "history-menu" => ReedlineEvent::Menu(HISTORY_MENU.to_string()),
        "menu-next" => ReedlineEvent::MenuNext,
        "menu-previous" => ReedlineEvent::MenuPrevious,
        "menu-accept" => ReedlineEvent::MenuAccept,
        "enter" => ReedlineEvent::Enter,
        "submit" => ReedlineEvent::Submit,
        "escape" => ReedlineEvent::Esc,
        "clear-screen" => ReedlineEvent::ClearScreen,
        "exit" => ReedlineEvent::CtrlD,
        "interrupt" => ReedlineEvent::CtrlC,
        "history-next" => ReedlineEvent::NextHistory,
        "history-previous" => ReedlineEvent::PreviousHistory,
        "insert-newline" => ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
        _ => return None,
    })
}

/// Merge provider bindings into `kb` (already holding the shell's own
/// bindings — providers registered later win). Returns warnings.
pub fn merge(kb: &mut Keybindings, providers: &[Box<dyn KeymapProvider>]) -> Vec<String> {
    let mut seen: HashMap<(KeyCode, KeyModifiers), usize> = HashMap::new();
    let mut warnings = Vec::new();
    for (i, p) in providers.iter().enumerate() {
        for (key_desc, event_name) in p.bindings() {
            let Some(key) = parse_key(&key_desc) else {
                warnings.push(format!("unknown key: {key_desc}"));
                continue;
            };
            let Some(event) = parse_event(&event_name) else {
                warnings.push(format!("unknown event: {event_name}"));
                continue;
            };
            if let Some(prev) = seen.get(&key)
                && *prev != i
            {
                warnings.push(format!(
                    "keybinding conflict: {key_desc} (provider #{prev} overridden by #{i})"
                ));
            }
            seen.insert(key, i);
            kb.add_binding(key.1, key.0, event);
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use brish_plugin::KeymapProvider;

    #[test]
    fn parses_keys() {
        assert_eq!(parse_key("tab"), Some((KeyCode::Tab, KeyModifiers::NONE)));
        assert_eq!(
            parse_key("CTRL-x"),
            Some((KeyCode::Char('x'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key("alt-1"),
            Some((KeyCode::Char('1'), KeyModifiers::ALT))
        );
        assert_eq!(
            parse_key("shift-tab"),
            Some((KeyCode::BackTab, KeyModifiers::SHIFT))
        );
        assert_eq!(
            parse_key("g"),
            Some((KeyCode::Char('g'), KeyModifiers::NONE))
        );
        assert_eq!(parse_key("up"), Some((KeyCode::Up, KeyModifiers::NONE)));
        assert_eq!(parse_key("ctrl-f13"), None, "no key after modifier");
        assert_eq!(parse_key("f13"), None, "function keys not supported");
        assert_eq!(parse_key(""), None);
    }

    #[test]
    fn parses_events() {
        assert!(matches!(
            parse_event("menu"),
            Some(ReedlineEvent::Menu(m)) if m == MENU_NAME
        ));
        assert!(matches!(
            parse_event("menu-next"),
            Some(ReedlineEvent::MenuNext)
        ));
        assert!(matches!(parse_event("escape"), Some(ReedlineEvent::Esc)));
        assert!(matches!(parse_event("exit"), Some(ReedlineEvent::CtrlD)));
        assert!(matches!(
            parse_event("insert-newline"),
            Some(ReedlineEvent::Edit(_))
        ));
        assert_eq!(parse_event("warp-speed"), None);
    }

    struct KP(&'static [(&'static str, &'static str)]);
    impl KeymapProvider for KP {
        fn bindings(&self) -> Vec<(String, String)> {
            self.0
                .iter()
                .map(|(k, e)| (k.to_string(), e.to_string()))
                .collect()
        }
    }

    #[test]
    fn merge_applies_valid_bindings() {
        let mut kb = Keybindings::empty();
        let providers: Vec<Box<dyn KeymapProvider>> =
            vec![Box::new(KP(&[("ctrl-g", "menu-next"), ("x", "enter")]))];
        let warnings = merge(&mut kb, &providers);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(matches!(
            kb.find_binding(KeyModifiers::CONTROL, KeyCode::Char('g')),
            Some(ReedlineEvent::MenuNext)
        ));
        assert!(matches!(
            kb.find_binding(KeyModifiers::NONE, KeyCode::Char('x')),
            Some(ReedlineEvent::Enter)
        ));
    }

    #[test]
    fn merge_warns_on_conflict_and_unknown() {
        let mut kb = Keybindings::empty();
        let providers: Vec<Box<dyn KeymapProvider>> = vec![
            Box::new(KP(&[("tab", "escape")])),
            Box::new(KP(&[
                ("tab", "menu"),
                ("bogus-key", "enter"),
                ("g", "warp"),
            ])),
        ];
        let warnings = merge(&mut kb, &providers);
        assert_eq!(
            warnings.iter().filter(|w| w.contains("conflict")).count(),
            1
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("unknown key: bogus-key"))
        );
        assert!(warnings.iter().any(|w| w.contains("unknown event: warp")));
        // last provider wins for the contested key
        assert!(matches!(
            kb.find_binding(KeyModifiers::NONE, KeyCode::Tab),
            Some(ReedlineEvent::Menu(m)) if m == MENU_NAME
        ));
    }
}
