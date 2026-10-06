//! Built-in behavioral plugins shipped with briSH (prompt catalog
//! lives in `brish-theme`).

pub mod announce;

use crate::CatalogEntry;
use announce::AnnounceCd;

/// Behavioral catalog, in registration order.
pub fn catalog() -> Vec<CatalogEntry> {
    vec![CatalogEntry {
        default_enabled: false,
        plugin: Box::new(AnnounceCd),
    }]
}
