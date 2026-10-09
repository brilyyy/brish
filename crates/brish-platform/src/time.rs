//! Local time utilities using chrono.

use chrono::{Local, Timelike};

/// Returns local time as (hour, minute, second).
pub fn localtime_hms() -> Option<(u32, u32, u32)> {
    let now = Local::now();
    Some((now.hour(), now.minute(), now.second()))
}

/// Returns local time as (hour, minute).
pub fn localtime_hm() -> Option<(u32, u32)> {
    let now = Local::now();
    Some((now.hour(), now.minute()))
}
