//! Window placement per monitor configuration (CORE-APP-002), as a model without a window.
//!
//! A *monitor configuration* is the set of monitors with their rectangles and scale factors; its
//! [`configuration_key`] does not depend on the order the platform lists them in. Placements
//! (position and size in physical pixels, maximized) are saved per configuration and per window
//! identifier, most recent configuration first. [`WindowStates::restore`] looks the window up
//! in the current configuration, else takes its most recent placement from any configuration,
//! and clamps it: the window lands entirely on the monitor it overlaps most, or centered on the
//! primary monitor when it overlaps none. [`rescale`] keeps a window's logical size when it
//! moves to a monitor with another scale factor. The states are a TOML table written through
//! the settings-file functions.

use std::path::Path;

use arc_base::Result;
use toml::{Table, Value};

use crate::settings_file::{self, SetAside};

/// The most monitors a configuration key describes; further monitors are ignored.
pub const MAX_MONITORS: usize = 16;
/// The most configurations kept; the least recently saved one is dropped first.
pub const MAX_CONFIGURATIONS: usize = 32;
/// The most windows kept per configuration.
pub const MAX_WINDOWS: usize = 16;
/// The longest window identifier, in bytes.
pub const MAX_WINDOW_ID_BYTES: usize = 64;
/// The smallest restored window side in physical pixels (smaller monitors limit it further).
pub const MIN_SIDE: u32 = 160;
/// The largest coordinate magnitude accepted, in physical pixels.
pub const MAX_COORDINATE: i64 = 1 << 20;
/// The scale factors accepted; others count as 1.
pub const SCALE_RANGE: (f64, f64) = (0.25, 16.0);

/// A monitor as the platform reports it, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    /// The top-left corner on the virtual desktop.
    pub position: [i32; 2],
    /// The size; zero sides make the monitor unusable.
    pub size: [u32; 2],
    /// The scale factor (physical pixels per logical point).
    pub scale: f64,
}

/// A window's placement in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The top-left corner of the window.
    pub position: [i32; 2],
    /// The inner size.
    pub size: [u32; 2],
    /// Whether the window is maximized; position and size are then the restored geometry.
    pub maximized: bool,
}

/// A scale factor in [`SCALE_RANGE`]; anything else is 1.
#[must_use]
pub fn clean_scale(scale: f64) -> f64 {
    if scale.is_finite() && (SCALE_RANGE.0..=SCALE_RANGE.1).contains(&scale) {
        scale
    } else {
        1.0
    }
}

/// The key of a monitor configuration: each usable monitor as `WxH+X+Y@S` (scale with three
/// decimals), sorted and joined by `;`. At most [`MAX_MONITORS`] monitors count.
#[must_use]
pub fn configuration_key(monitors: &[Monitor]) -> String {
    let mut parts: Vec<String> = monitors
        .iter()
        .filter(|m| m.size[0] > 0 && m.size[1] > 0)
        .take(MAX_MONITORS)
        .map(|m| {
            format!(
                "{}x{}+{}+{}@{:.3}",
                m.size[0],
                m.size[1],
                m.position[0],
                m.position[1],
                clean_scale(m.scale)
            )
        })
        .collect();
    parts.sort();
    parts.join(";")
}

/// True when `id` is a window identifier: 1 to [`MAX_WINDOW_ID_BYTES`] bytes of lowercase
/// ASCII letters, digits, `.`, `-` and `_`, starting with a letter.
#[must_use]
pub fn is_window_id(id: &str) -> bool {
    id.len() <= MAX_WINDOW_ID_BYTES
        && id.starts_with(|c: char| c.is_ascii_lowercase())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
}

fn rect(m: &Monitor) -> [i64; 4] {
    let x = i64::from(m.position[0]);
    let y = i64::from(m.position[1]);
    [x, y, x + i64::from(m.size[0]), y + i64::from(m.size[1])]
}

fn overlap(a: [i64; 4], b: [i64; 4]) -> i64 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0);
    w.saturating_mul(h)
}

/// Clamps `placement` onto the usable monitors: onto the monitor it overlaps most, entirely
/// inside it, or centered on `monitors[primary]` (the first usable monitor when that index is
/// not usable) when it overlaps none. Sizes shrink to the monitor and grow to [`MIN_SIDE`] where
/// the monitor allows. `None` when no monitor is usable.
#[must_use]
pub fn clamp(placement: Placement, monitors: &[Monitor], primary: usize) -> Option<Placement> {
    let usable: Vec<&Monitor> = monitors
        .iter()
        .take(MAX_MONITORS)
        .filter(|m| m.size[0] > 0 && m.size[1] > 0)
        .collect();
    let x = i64::from(placement.position[0]);
    let y = i64::from(placement.position[1]);
    let window = [
        x,
        y,
        x + i64::from(placement.size[0]),
        y + i64::from(placement.size[1]),
    ];
    let best = usable
        .iter()
        .map(|m| (overlap(window, rect(m)), *m))
        .filter(|(area, _)| *area > 0)
        .max_by_key(|(area, _)| *area)
        .map(|(_, m)| m);
    let fallback = monitors
        .get(primary)
        .filter(|m| m.size[0] > 0 && m.size[1] > 0)
        .or_else(|| usable.first().copied())?;
    let (monitor, centered) = best.map_or((fallback, true), |m| (m, false));
    let side = |want: u32, room: u32| want.max(MIN_SIDE.min(room)).min(room);
    let size = [
        side(placement.size[0], monitor.size[0]),
        side(placement.size[1], monitor.size[1]),
    ];
    let [left, top, right, bottom] = rect(monitor);
    let place = |at: i64, len: u32, lo: i64, hi: i64| {
        let len = i64::from(len);
        let at = if centered {
            lo + (hi - lo - len) / 2
        } else {
            at
        };
        at.clamp(lo, hi - len)
    };
    let px = place(x, size[0], left, right);
    let py = place(y, size[1], top, bottom);
    Some(Placement {
        position: [
            i32::try_from(px).unwrap_or(monitor.position[0]),
            i32::try_from(py).unwrap_or(monitor.position[1]),
        ],
        size,
        maximized: placement.maximized,
    })
}

/// The physical size that keeps a window's logical size when its scale factor changes from
/// `old` to `new` (both cleaned by [`clean_scale`]), at least 1 per side.
#[must_use]
pub fn rescale(size: [u32; 2], old: f64, new: f64) -> [u32; 2] {
    let factor = clean_scale(new) / clean_scale(old);
    size.map(|side| {
        let scaled = (f64::from(side) * factor).round();
        // The factor is at most 64, so the value fits after the clamp.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped"
        )]
        let side = scaled.clamp(1.0, f64::from(u32::MAX)) as u32;
        side
    })
}

/// Saved placements per monitor configuration; see the [module documentation](self).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowStates {
    /// Configurations, most recently saved first, each with its windows in save order.
    configurations: Vec<(String, Vec<(String, Placement)>)>,
}

impl WindowStates {
    /// No saved placement.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Saves `placement` of window `id` for the configuration of `monitors`, which becomes the
    /// most recent one. A malformed identifier or coordinates beyond [`MAX_COORDINATE`] are
    /// ignored; the oldest configuration or window is dropped beyond the limits.
    pub fn save(&mut self, monitors: &[Monitor], id: &str, placement: Placement) {
        let far = |v: i32| i64::from(v).abs() > MAX_COORDINATE;
        let big = |v: u32| i64::from(v) > MAX_COORDINATE;
        if !is_window_id(id)
            || placement.position.iter().any(|v| far(*v))
            || placement.size.iter().any(|v| big(*v) || *v == 0)
        {
            return;
        }
        let key = configuration_key(monitors);
        let mut windows = match self.configurations.iter().position(|(k, _)| *k == key) {
            Some(at) => self.configurations.remove(at).1,
            None => Vec::new(),
        };
        windows.retain(|(w, _)| w != id);
        windows.push((id.to_owned(), placement));
        if windows.len() > MAX_WINDOWS {
            windows.remove(0);
        }
        self.configurations.insert(0, (key, windows));
        self.configurations.truncate(MAX_CONFIGURATIONS);
    }

    /// The placement saved for window `id` in exactly this configuration, unclamped.
    #[must_use]
    pub fn saved(&self, monitors: &[Monitor], id: &str) -> Option<Placement> {
        let key = configuration_key(monitors);
        self.configurations
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, w)| w.iter().find(|(i, _)| i == id))
            .map(|(_, p)| *p)
    }

    /// The placement to open window `id` with on `monitors`: the one saved for this
    /// configuration, else the most recent one from any configuration, clamped with [`clamp`].
    /// `None` when nothing was saved for the window or no monitor is usable; the caller then
    /// uses its default placement.
    #[must_use]
    pub fn restore(&self, monitors: &[Monitor], primary: usize, id: &str) -> Option<Placement> {
        let found = self.saved(monitors, id).or_else(|| {
            self.configurations
                .iter()
                .find_map(|(_, w)| w.iter().find(|(i, _)| i == id).map(|(_, p)| *p))
        })?;
        clamp(found, monitors, primary)
    }

    /// The states as a TOML table: `configurations = [{ key, windows = [{ id, x, y, width,
    /// height, maximized }] }]`, most recent first.
    #[must_use]
    pub fn to_table(&self) -> Table {
        let configurations = self
            .configurations
            .iter()
            .map(|(key, windows)| {
                let windows = windows
                    .iter()
                    .map(|(id, p)| {
                        let mut t = Table::new();
                        t.insert("id".into(), Value::String(id.clone()));
                        t.insert("x".into(), Value::Integer(p.position[0].into()));
                        t.insert("y".into(), Value::Integer(p.position[1].into()));
                        t.insert("width".into(), Value::Integer(p.size[0].into()));
                        t.insert("height".into(), Value::Integer(p.size[1].into()));
                        t.insert("maximized".into(), Value::Boolean(p.maximized));
                        Value::Table(t)
                    })
                    .collect();
                let mut t = Table::new();
                t.insert("key".into(), Value::String(key.clone()));
                t.insert("windows".into(), Value::Array(windows));
                Value::Table(t)
            })
            .collect();
        let mut table = Table::new();
        table.insert("configurations".into(), Value::Array(configurations));
        table
    }

    /// Reads states from a table. Malformed entries, duplicates and entries beyond the limits
    /// are skipped.
    #[must_use]
    pub fn from_table(table: &Table) -> Self {
        fn list(v: Option<&Value>) -> &[Value] {
            v.and_then(Value::as_array).map_or(&[][..], Vec::as_slice)
        }
        let mut states = Self::new();
        for item in list(table.get("configurations")).iter() {
            if states.configurations.len() >= MAX_CONFIGURATIONS {
                break;
            }
            let Some(key) = item.get("key").and_then(Value::as_str) else {
                continue;
            };
            if key.len() > MAX_MONITORS * 64 || states.configurations.iter().any(|(k, _)| k == key)
            {
                continue;
            }
            let mut windows: Vec<(String, Placement)> = Vec::new();
            for w in list(item.get("windows")).iter().take(MAX_WINDOWS * 2) {
                if let Some((id, p)) = window_of(w)
                    && windows.len() < MAX_WINDOWS
                    && !windows.iter().any(|(i, _)| *i == id)
                {
                    windows.push((id, p));
                }
            }
            states.configurations.push((key.to_owned(), windows));
        }
        states
    }

    /// Loads the states from `path`; a malformed file is set aside as for settings files.
    ///
    /// # Errors
    /// As [`settings_file::load`].
    pub fn load(path: &Path) -> Result<(Self, Option<SetAside>)> {
        let loaded = settings_file::load(path)?;
        Ok((Self::from_table(&loaded.table), loaded.set_aside))
    }

    /// Saves the states atomically.
    ///
    /// # Errors
    /// As [`settings_file::save`].
    pub fn save_to(&self, path: &Path) -> Result<()> {
        settings_file::save(path, &self.to_table())
    }
}

fn window_of(item: &Value) -> Option<(String, Placement)> {
    let id = item.get("id")?.as_str()?;
    let int = |name: &str| item.get(name)?.as_integer();
    let coordinate = |name: &str| {
        int(name)
            .filter(|v| v.abs() <= MAX_COORDINATE)
            .and_then(|v| i32::try_from(v).ok())
    };
    let side = |name: &str| {
        int(name)
            .filter(|v| (1..=MAX_COORDINATE).contains(v))
            .and_then(|v| u32::try_from(v).ok())
    };
    is_window_id(id).then_some(())?;
    Some((
        id.to_owned(),
        Placement {
            position: [coordinate("x")?, coordinate("y")?],
            size: [side("width")?, side("height")?],
            maximized: item.get("maximized")?.as_bool()?,
        },
    ))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::testutil::dir;

    fn monitor(x: i32, y: i32, w: u32, h: u32, scale: f64) -> Monitor {
        Monitor {
            position: [x, y],
            size: [w, h],
            scale,
        }
    }

    fn at(x: i32, y: i32, w: u32, h: u32) -> Placement {
        Placement {
            position: [x, y],
            size: [w, h],
            maximized: false,
        }
    }

    fn inside(p: &Placement, m: &Monitor) -> bool {
        let [l, t, r, b] = rect(m);
        let x = i64::from(p.position[0]);
        let y = i64::from(p.position[1]);
        x >= l && y >= t && x + i64::from(p.size[0]) <= r && y + i64::from(p.size[1]) <= b
    }

    #[test]
    fn key_ignores_order_and_includes_scale() {
        let a = monitor(0, 0, 1920, 1080, 1.0);
        let b = monitor(1920, 0, 2560, 1440, 1.5);
        assert_eq!(configuration_key(&[a, b]), configuration_key(&[b, a]));
        let b2 = Monitor { scale: 2.0, ..b };
        assert_ne!(configuration_key(&[a, b]), configuration_key(&[a, b2]));
        assert_eq!(
            configuration_key(&[a]),
            "1920x1080+0+0@1.000",
            "non-finite scales count as 1"
        );
        let nan = Monitor {
            scale: f64::NAN,
            ..a
        };
        assert_eq!(configuration_key(&[nan]), configuration_key(&[a]));
    }

    #[test]
    fn save_restore_per_configuration_and_fallback_on_change() {
        let laptop = [monitor(0, 0, 1920, 1080, 1.0)];
        let docked = [laptop[0], monitor(1920, 0, 2560, 1440, 1.5)];
        let mut s = WindowStates::new();
        s.save(&docked, "main", at(2000, 100, 1600, 900));
        s.save(&laptop, "main", at(10, 20, 800, 600));
        assert_eq!(s.restore(&laptop, 0, "main"), Some(at(10, 20, 800, 600)));
        assert_eq!(
            s.restore(&docked, 0, "main"),
            Some(at(2000, 100, 1600, 900))
        );
        // A new configuration takes the most recent placement, clamped: the laptop one.
        let other = [monitor(-1280, 0, 1280, 1024, 1.0)];
        let p = s.restore(&other, 0, "main").unwrap();
        assert!(inside(&p, &other[0]), "{p:?}");
        assert_eq!(p.size, [800, 600]);
        // The window that was on the right monitor opens on the primary one when undocked.
        let mut only_docked = WindowStates::new();
        only_docked.save(&docked, "main", at(2000, 100, 1600, 900));
        let p = only_docked.restore(&laptop, 0, "main").unwrap();
        assert!(inside(&p, &laptop[0]));
        assert_eq!(p.position, [160, 90], "centered on the primary monitor");
        assert_eq!(s.restore(&laptop, 0, "secondary"), None);
        assert_eq!(s.restore(&[], 0, "main"), None);
    }

    #[test]
    fn clamp_shrinks_oversized_windows_and_keeps_small_monitors_usable() {
        let m = monitor(0, 0, 100, 80, 1.0);
        let p = clamp(at(-50, -50, 5000, 5000), &[m], 0).unwrap();
        assert_eq!(p, at(0, 0, 100, 80));
        let big = monitor(0, 0, 1000, 1000, 1.0);
        let p = clamp(at(10, 10, 1, 1), &[big], 0).unwrap();
        assert_eq!(p.size, [MIN_SIDE, MIN_SIDE]);
        // The primary index falls back to the first usable monitor.
        let zero = monitor(0, 0, 0, 0, 1.0);
        let p = clamp(at(50_000, 50_000, 300, 300), &[zero, big], 0).unwrap();
        assert!(inside(&p, &big));
    }

    #[test]
    fn rescale_keeps_the_logical_size() {
        assert_eq!(rescale([800, 600], 1.0, 2.0), [1600, 1200]);
        assert_eq!(rescale([1600, 1200], 2.0, 1.0), [800, 600]);
        assert_eq!(rescale([801, 601], 1.5, 1.0), [534, 401]);
        assert_eq!(rescale([800, 600], f64::NAN, 1e300), [800, 600]);
        assert_eq!(rescale([1, 1], 16.0, 0.25), [1, 1]);
    }

    #[test]
    fn limits_and_malformed_input_are_ignored() {
        let m = [monitor(0, 0, 1920, 1080, 1.0)];
        let mut s = WindowStates::new();
        s.save(&m, "Main", at(0, 0, 10, 10));
        s.save(&m, "main", at(i32::MAX, 0, 10, 10));
        s.save(&m, "main", at(0, 0, 0, 10));
        assert_eq!(s, WindowStates::new());
        for n in 0..(MAX_WINDOWS + 3) {
            s.save(&m, &format!("w{n}"), at(0, 0, 10, 10));
        }
        assert_eq!(s.configurations[0].1.len(), MAX_WINDOWS);
        assert_eq!(s.saved(&m, "w0"), None, "the oldest window is dropped");
        for n in 0..(MAX_CONFIGURATIONS + 3) {
            let x = i32::try_from(n).unwrap() * 2000;
            s.save(&[monitor(x, 0, 100, 100, 1.0)], "main", at(x, 0, 10, 10));
        }
        assert_eq!(s.configurations.len(), MAX_CONFIGURATIONS);
    }

    #[test]
    fn table_round_trip_and_hostile_tables() {
        let m = [monitor(0, 0, 1920, 1080, 1.25)];
        let mut s = WindowStates::new();
        s.save(&m, "main", at(-5, 7, 640, 480));
        s.save(
            &m,
            "secondary.1",
            Placement {
                maximized: true,
                ..at(100, 100, 300, 200)
            },
        );
        let d = dir("window-state");
        let path = d.join("windows.toml");
        s.save_to(&path).unwrap();
        let (back, aside) = WindowStates::load(&path).unwrap();
        assert!(aside.is_none());
        assert_eq!(back, s);
        let hostile: Table = toml::from_str(
            r#"
            configurations = [
              { key = "k", windows = [
                { id = "ok", x = 1, y = 2, width = 3, height = 4, maximized = false },
                { id = "ok", x = 9, y = 9, width = 9, height = 9, maximized = false },
                { id = "BAD", x = 1, y = 2, width = 3, height = 4, maximized = false },
                { id = "big", x = 9999999999, y = 2, width = 3, height = 4, maximized = false },
                { id = "neg", x = 1, y = 2, width = -3, height = 4, maximized = false },
                "text",
              ] },
              { key = "k", windows = [] },
              { windows = [] },
              7,
            ]
            "#,
        )
        .unwrap();
        let s = WindowStates::from_table(&hostile);
        assert_eq!(s.configurations.len(), 1);
        assert_eq!(
            s.configurations[0].1,
            vec![("ok".to_owned(), at(1, 2, 3, 4))]
        );
        let _ = std::fs::remove_dir_all(d);
    }

    fn monitors() -> impl Strategy<Value = Vec<Monitor>> {
        prop::collection::vec(
            (
                -100_000i32..100_000,
                -100_000i32..100_000,
                0u32..10_000,
                0u32..10_000,
                prop_oneof![Just(f64::NAN), Just(1e300), 0.1f64..20.0],
            )
                .prop_map(|(x, y, w, h, s)| monitor(x, y, w, h, s)),
            0..5,
        )
    }

    fn placement() -> impl Strategy<Value = Placement> {
        (
            any::<i32>(),
            any::<i32>(),
            any::<u32>(),
            any::<u32>(),
            any::<bool>(),
        )
            .prop_map(|(x, y, w, h, maximized)| Placement {
                position: [x, y],
                size: [w, h],
                maximized,
            })
    }

    proptest! {
        #[test]
        fn restored_windows_lie_on_a_usable_monitor(
            saved_on in monitors(),
            now in monitors(),
            p in placement(),
            primary in 0usize..6,
        ) {
            let mut s = WindowStates::new();
            s.save(&saved_on, "main", p);
            let usable: Vec<&Monitor> =
                now.iter().filter(|m| m.size[0] > 0 && m.size[1] > 0).collect();
            match s.restore(&now, primary, "main") {
                Some(r) => {
                    prop_assert!(usable.iter().any(|m| inside(&r, m)), "{r:?} {now:?}");
                    prop_assert_eq!(r.maximized, p.maximized);
                    // Unchanged configuration and a placement that fits: restored exactly.
                    if configuration_key(&saved_on) == configuration_key(&now)
                        && usable.iter().any(|m| inside(&p, m) && p.size.iter().all(|v| *v >= MIN_SIDE))
                    {
                        prop_assert_eq!(r, p);
                    }
                }
                None => prop_assert!(
                    usable.is_empty() || s.saved(&saved_on, "main").is_none()
                ),
            }
            let back = WindowStates::from_table(&s.to_table());
            prop_assert_eq!(back, s);
        }
    }
}
