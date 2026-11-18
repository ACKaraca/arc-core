//! Key bindings and chords (CORE-UI-004).
//!
//! A binding maps a key sequence of one to [`MAX_STROKES`] strokes (`"Ctrl+S"`,
//! `"Ctrl+K Ctrl+C"`) to a command identifier. [`Keymap::build`] parses the bindings read from
//! settings, keeps the first binding of each sequence and reports the rest: same-sequence
//! conflicts, prefix conflicts, unknown commands and malformed text. [`ChordResolver`] turns
//! key presses into intents; the time comes from the caller, so a chord expires after
//! [`CHORD_TIMEOUT_MILLIS`] of the caller's clock.

use std::fmt;

use arc_base::{Error, Result};
use arc_command::Registry;
use arc_command::spec::Surface;
use egui::Key;
use serde::{Deserialize, Serialize};

use crate::command::Intent;
use crate::error::codes;

/// The most strokes in one key sequence.
pub const MAX_STROKES: usize = 4;
/// The most bindings in one key map.
pub const MAX_BINDINGS: usize = 1024;
/// The longest binding text, in bytes.
pub const MAX_KEYS_BYTES: usize = 128;
/// A pending chord is dropped when the next stroke comes later than this.
pub const CHORD_TIMEOUT_MILLIS: u64 = 1500;

/// One stroke: a key with modifiers. `meta` is the Command key on macOS and the logo key
/// elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    /// Control.
    pub ctrl: bool,
    /// Alt (Option).
    pub alt: bool,
    /// Shift.
    pub shift: bool,
    /// Command or logo key.
    pub meta: bool,
    /// The key.
    pub key: Key,
}

impl KeyCombo {
    /// The stroke of a key-press event; `None` for releases, repeats and other events.
    #[must_use]
    pub fn from_event(event: &egui::Event) -> Option<Self> {
        match event {
            egui::Event::Key {
                key,
                pressed: true,
                repeat: false,
                modifiers,
                ..
            } => Some(Self {
                ctrl: modifiers.ctrl,
                alt: modifiers.alt,
                shift: modifiers.shift,
                meta: modifiers.mac_cmd,
                key: *key,
            }),
            _ => None,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        let mut combo = Self {
            ctrl: false,
            alt: false,
            shift: false,
            meta: false,
            key: Key::A,
        };
        let mut parts: Vec<&str> = text.split('+').collect();
        let key = parts.pop()?;
        for part in parts {
            let flag = match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut combo.ctrl,
                "alt" | "option" => &mut combo.alt,
                "shift" => &mut combo.shift,
                "meta" | "cmd" | "command" | "super" => &mut combo.meta,
                _ => return None,
            };
            if *flag {
                return None;
            }
            *flag = true;
        }
        combo.key = Key::from_name(key)?;
        Some(combo)
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
            (self.meta, "Meta+"),
        ] {
            if on {
                f.write_str(name)?;
            }
        }
        f.write_str(self.key.name())
    }
}

/// One to [`MAX_STROKES`] strokes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySequence(Vec<KeyCombo>);

impl KeySequence {
    /// Parses strokes separated by spaces, each modifiers and a key joined by `+`; modifier
    /// names are case-insensitive, key names are egui's (`S`, `F5`, `Enter`, `Plus`).
    ///
    /// # Errors
    /// `ui.keys.invalid` for empty, too long or malformed text.
    pub fn parse(text: &str) -> Result<Self> {
        let invalid = || Error::new(codes::KEYS_INVALID).with_arg("keys", text);
        if text.len() > MAX_KEYS_BYTES {
            return Err(invalid());
        }
        let strokes: Vec<KeyCombo> = text
            .split_whitespace()
            .map(KeyCombo::parse)
            .collect::<Option<_>>()
            .ok_or_else(invalid)?;
        if strokes.is_empty() || strokes.len() > MAX_STROKES {
            return Err(invalid());
        }
        Ok(Self(strokes))
    }

    /// The strokes.
    #[must_use]
    pub fn strokes(&self) -> &[KeyCombo] {
        &self.0
    }

    fn starts_with(&self, prefix: &[KeyCombo]) -> bool {
        self.0.starts_with(prefix)
    }
}

impl fmt::Display for KeySequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, stroke) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(" ")?;
            }
            write!(f, "{stroke}")?;
        }
        Ok(())
    }
}

/// A binding as stored in settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// The key sequence text.
    pub keys: String,
    /// The command identifier.
    pub command: String,
}

/// Validated bindings: no sequence is bound twice and none is the prefix of another.
#[derive(Debug, Clone, Default)]
pub struct Keymap {
    bindings: Vec<(KeySequence, String)>,
}

impl Keymap {
    /// Parses `entries` in order and keeps each binding that is valid and does not conflict
    /// with a binding kept before it. Returns the map and one error per dropped entry:
    /// `ui.keys.invalid`, `ui.command.unknown` (not registered or not exposed on the UI
    /// surface), `ui.keys.conflict` (the same sequence is bound to another command; a repeat
    /// for the same command is dropped silently), `ui.keys.prefix_conflict`, and one
    /// `ui.keys.too_many` for the entries beyond [`MAX_BINDINGS`].
    #[must_use]
    pub fn build(entries: &[Binding], registry: &Registry) -> (Self, Vec<Error>) {
        let mut map = Self::default();
        let mut problems = Vec::new();
        for entry in entries {
            if map.bindings.len() == MAX_BINDINGS {
                problems.push(Error::new(codes::KEYS_TOO_MANY).with_detail(
                    "dropped",
                    i64::try_from(entries.len() - MAX_BINDINGS).unwrap_or(i64::MAX),
                ));
                break;
            }
            let sequence = match KeySequence::parse(&entry.keys) {
                Ok(sequence) => sequence,
                Err(error) => {
                    problems.push(error);
                    continue;
                }
            };
            if !registry
                .get(&entry.command)
                .is_some_and(|spec| spec.exposure().contains(&Surface::Ui))
            {
                problems.push(
                    Error::new(codes::COMMAND_UNKNOWN).with_arg("command", entry.command.as_str()),
                );
                continue;
            }
            if let Some((_, other)) = map.bindings.iter().find(|(s, _)| *s == sequence) {
                if *other != entry.command {
                    problems.push(
                        Error::new(codes::KEYS_CONFLICT)
                            .with_arg("keys", sequence.to_string())
                            .with_arg("command", entry.command.as_str())
                            .with_arg("other", other.as_str()),
                    );
                }
                continue;
            }
            if let Some((other_keys, other)) = map
                .bindings
                .iter()
                .find(|(s, _)| s.starts_with(&sequence.0) || sequence.starts_with(&s.0))
            {
                problems.push(
                    Error::new(codes::KEYS_PREFIX_CONFLICT)
                        .with_arg("keys", sequence.to_string())
                        .with_arg("command", entry.command.as_str())
                        .with_arg("other_keys", other_keys.to_string())
                        .with_arg("other", other.as_str()),
                );
                continue;
            }
            map.bindings.push((sequence, entry.command.clone()));
        }
        (map, problems)
    }

    /// The bindings in the order they were kept.
    pub fn iter(&self) -> impl Iterator<Item = (&KeySequence, &str)> + '_ {
        self.bindings.iter().map(|(s, c)| (s, c.as_str()))
    }

    /// The display text of the first binding of `command`.
    #[must_use]
    pub fn display(&self, command: &str) -> Option<String> {
        self.bindings
            .iter()
            .find(|(_, c)| c == command)
            .map(|(s, _)| s.to_string())
    }
}

/// What a key press did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyOutcome {
    /// A binding completed.
    Invoke(Intent),
    /// The press started or continued a chord.
    Pending,
    /// No binding matches; a pending chord was dropped.
    Unbound,
}

/// The chord state machine.
#[derive(Debug, Clone, Default)]
pub struct ChordResolver {
    pending: Vec<KeyCombo>,
    last_millis: u64,
}

impl ChordResolver {
    /// Feeds one stroke at `now_millis` of the caller's clock. A pending chord whose last
    /// stroke is more than [`CHORD_TIMEOUT_MILLIS`] old is dropped first; a clock that runs
    /// backwards counts as no time passed.
    pub fn press(&mut self, keymap: &Keymap, combo: KeyCombo, now_millis: u64) -> KeyOutcome {
        self.expire(now_millis);
        self.pending.push(combo);
        self.last_millis = now_millis;
        if let Some((_, command)) = keymap.bindings.iter().find(|(s, _)| s.0 == self.pending) {
            self.pending.clear();
            return KeyOutcome::Invoke(Intent {
                command: command.clone(),
            });
        }
        if keymap
            .bindings
            .iter()
            .any(|(s, _)| s.starts_with(&self.pending))
        {
            return KeyOutcome::Pending;
        }
        self.pending.clear();
        KeyOutcome::Unbound
    }

    /// Drops a pending chord that timed out at `now_millis`.
    pub fn expire(&mut self, now_millis: u64) {
        if now_millis.saturating_sub(self.last_millis) > CHORD_TIMEOUT_MILLIS {
            self.pending.clear();
        }
    }

    /// The strokes of the pending chord.
    #[must_use]
    pub fn pending(&self) -> &[KeyCombo] {
        &self.pending
    }

    /// Drops the pending chord.
    pub fn cancel(&mut self) {
        self.pending.clear();
    }

    /// Feeds every key press of `events` and returns the intents of completed bindings.
    pub fn handle_events(
        &mut self,
        keymap: &Keymap,
        events: &[egui::Event],
        now_millis: u64,
    ) -> Vec<Intent> {
        events
            .iter()
            .filter_map(KeyCombo::from_event)
            .filter_map(|combo| match self.press(keymap, combo, now_millis) {
                KeyOutcome::Invoke(intent) => Some(intent),
                KeyOutcome::Pending | KeyOutcome::Unbound => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::tests::registry;
    use proptest::prelude::*;

    fn b(keys: &str, command: &str) -> Binding {
        Binding {
            keys: keys.into(),
            command: command.into(),
        }
    }

    fn combo(text: &str) -> KeyCombo {
        KeyCombo::parse(text).unwrap()
    }

    #[test]
    fn parse_and_display_round_trip() {
        let s = KeySequence::parse("shift+ctrl+s").unwrap();
        assert_eq!(s.to_string(), "Ctrl+Shift+S");
        let chord = KeySequence::parse("Ctrl+K   Ctrl+C").unwrap();
        assert_eq!(chord.to_string(), "Ctrl+K Ctrl+C");
        assert_eq!(KeySequence::parse(&chord.to_string()).unwrap(), chord);
        for bad in [
            "",
            "Ctrl+",
            "Hyper+S",
            "Ctrl+Ctrl+S",
            "A B C D E",
            "NoSuchKey",
            "+",
        ] {
            assert_eq!(
                KeySequence::parse(bad).unwrap_err().code(),
                "ui.keys.invalid",
                "{bad}"
            );
        }
        assert!(KeySequence::parse(&"A ".repeat(100)).is_err());
    }

    #[test]
    fn conflicts_are_detected_and_the_first_binding_wins() {
        let registry = registry();
        let (map, problems) = Keymap::build(
            &[
                b("Ctrl+S", "demo.file.save"),
                b("ctrl+s", "demo.file.open"),
                b("Ctrl+S", "demo.file.save"),
                b("Ctrl+K Ctrl+O", "demo.file.open"),
                b("Ctrl+K", "demo.edit.undo"),
                b("Ctrl+K Ctrl+O Ctrl+X", "demo.edit.undo"),
                b("Ctrl+Z", "demo.hidden.cmd"),
                b("Ctrl+Z", "demo.no.such"),
                b("Ctrl+", "demo.edit.undo"),
                b("Ctrl+Z", "demo.edit.undo"),
            ],
            &registry,
        );
        let codes: Vec<&str> = problems.iter().map(Error::code).collect();
        assert_eq!(
            codes,
            [
                "ui.keys.conflict",
                "ui.keys.prefix_conflict",
                "ui.keys.prefix_conflict",
                "ui.command.unknown",
                "ui.command.unknown",
                "ui.keys.invalid",
            ]
        );
        assert_eq!(map.iter().count(), 3);
        assert_eq!(map.display("demo.file.save").as_deref(), Some("Ctrl+S"));
        assert_eq!(
            map.display("demo.file.open").as_deref(),
            Some("Ctrl+K Ctrl+O")
        );
        assert_eq!(map.display("demo.edit.undo").as_deref(), Some("Ctrl+Z"));
    }

    #[test]
    fn bindings_deserialize_from_settings() {
        let text = "[[bindings]]\nkeys = \"Ctrl+S\"\ncommand = \"demo.file.save\"\n";
        #[derive(Deserialize)]
        struct File {
            bindings: Vec<Binding>,
        }
        let file: File = toml::from_str(text).unwrap();
        let (map, problems) = Keymap::build(&file.bindings, &registry());
        assert!(problems.is_empty());
        assert_eq!(map.display("demo.file.save").as_deref(), Some("Ctrl+S"));
    }

    #[test]
    fn too_many_bindings_are_reported_once() {
        let registry = registry();
        let entries: Vec<Binding> = (0..MAX_BINDINGS + 5)
            .map(|i| {
                // Distinct four-stroke sequences of the same length never prefix each other.
                let keys = format!(
                    "F{} F{} F{} F21",
                    1 + i % 20,
                    1 + (i / 20) % 20,
                    1 + (i / 400) % 20
                );
                b(&keys, "demo.file.save")
            })
            .collect();
        let (map, problems) = Keymap::build(&entries, &registry);
        assert_eq!(map.iter().count(), MAX_BINDINGS);
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].code(), "ui.keys.too_many");
    }

    #[test]
    fn chords_resolve_and_time_out_on_the_callers_clock() {
        let (map, _) = Keymap::build(
            &[
                b("Ctrl+S", "demo.file.save"),
                b("Ctrl+K Ctrl+O", "demo.file.open"),
            ],
            &registry(),
        );
        let mut r = ChordResolver::default();
        let save = KeyOutcome::Invoke(Intent {
            command: "demo.file.save".into(),
        });
        assert_eq!(r.press(&map, combo("Ctrl+S"), 0), save);
        assert_eq!(r.press(&map, combo("Ctrl+K"), 10), KeyOutcome::Pending);
        assert_eq!(
            r.press(&map, combo("Ctrl+O"), 10 + CHORD_TIMEOUT_MILLIS),
            KeyOutcome::Invoke(Intent {
                command: "demo.file.open".into()
            })
        );
        assert_eq!(r.press(&map, combo("Ctrl+K"), 5000), KeyOutcome::Pending);
        // Too late: the chord expired, and Ctrl+O alone is unbound.
        assert_eq!(
            r.press(&map, combo("Ctrl+O"), 5001 + CHORD_TIMEOUT_MILLIS),
            KeyOutcome::Unbound
        );
        assert!(r.pending().is_empty());
        // A wrong second stroke drops the chord; a clock running backwards does not expire it.
        assert_eq!(r.press(&map, combo("Ctrl+K"), 9000), KeyOutcome::Pending);
        assert_eq!(r.press(&map, combo("Ctrl+K"), 1), KeyOutcome::Unbound);
        assert_eq!(r.press(&map, combo("Ctrl+K"), 2), KeyOutcome::Pending);
        r.cancel();
        assert_eq!(r.press(&map, combo("Ctrl+S"), 3), save);
    }

    #[test]
    fn egui_key_events_become_intents() {
        let (map, _) = Keymap::build(&[b("Ctrl+S", "demo.file.save")], &registry());
        let press = |pressed, repeat| egui::Event::Key {
            key: Key::S,
            physical_key: None,
            pressed,
            repeat,
            modifiers: egui::Modifiers::CTRL,
        };
        let mut r = ChordResolver::default();
        let intents = r.handle_events(
            &map,
            &[press(true, false), press(false, false), press(true, true)],
            0,
        );
        assert_eq!(intents.len(), 1);
    }

    proptest! {
        #[test]
        fn parse_never_panics(text in "\\PC{0,200}") {
            let _ = KeySequence::parse(&text);
        }

        #[test]
        fn kept_bindings_never_conflict(keys in proptest::collection::vec(
            proptest::collection::vec(prop_oneof![Just("Ctrl+A"), Just("Ctrl+B"), Just("F1"), Just("Shift+F1")], 1..4), 0..40)) {
            let entries: Vec<Binding> = keys.iter().map(|k| b(&k.join(" "), "demo.file.save")).collect();
            let (map, _) = Keymap::build(&entries, &registry());
            let kept: Vec<&KeySequence> = map.iter().map(|(s, _)| s).collect();
            for (i, a) in kept.iter().enumerate() {
                for (j, c) in kept.iter().enumerate() {
                    if i != j {
                        prop_assert!(!a.starts_with(&c.0));
                    }
                }
            }
        }
    }
}
