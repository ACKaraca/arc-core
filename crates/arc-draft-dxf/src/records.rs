//! The records the import keeps for the parts of a file that the drafting model holds no object
//! for: class definitions, registered applications (APPID entries), dictionaries and the extended
//! data of created objects. Each is stored as a [`PreservedRecord`] (encoded by
//! [`crate::plan::encode_record`]) and decoded again by the export.
//!
//! Decoding is total: a record that does not have the expected shape is `None`, and the caller
//! reports it instead of writing a damaged group.

use std::collections::BTreeSet;

use arc_io_dxf::model::{
    AppId, Class, Dictionary, DictionaryEntry, Head, Object, ObjectKind, TableEntry, XData,
};
use arc_io_dxf::{Group, PreservedRecord, Value};

/// The record kind of a class definition.
pub const CLASS: &str = "CLASS";
/// The record kind of a registered application.
pub const APPID: &str = "APPID";
/// The record kind of a dictionary.
pub const DICTIONARY: &str = "DICTIONARY";
/// The record kind of extended data.
pub const XDATA: &str = "XDATA";

/// The groups of extended data: each application's group 1001 followed by its groups, in order.
#[must_use]
pub fn xdata_groups(xdata: &[XData]) -> Vec<Group> {
    let mut out = Vec::new();
    for x in xdata {
        out.push(Group::text(1001, x.app.clone()));
        out.extend(x.groups.iter().cloned());
    }
    out
}

/// The extended data of `groups`, which must start with group 1001; `None` otherwise or when a
/// group 1001 has no text.
#[must_use]
pub fn xdata_from_groups(groups: &[Group]) -> Option<Vec<XData>> {
    let mut out: Vec<XData> = Vec::new();
    for g in groups {
        if g.code == 1001 {
            out.push(XData {
                app: g.as_text()?.to_owned(),
                groups: Vec::new(),
            });
        } else {
            out.last_mut()?.groups.push(g.clone());
        }
    }
    Some(out)
}

/// The record of a class definition: its typed groups in the order the writer uses, then the
/// groups the model does not type.
#[must_use]
pub fn class_record(class: &Class) -> PreservedRecord {
    let mut groups = vec![
        Group::text(1, class.dxf_name.clone()),
        Group::text(2, class.class_name.clone()),
        Group::text(3, class.application.clone()),
        Group::int(90, i64::from(class.proxy_flags)),
    ];
    if let Some(count) = class.instance_count {
        groups.push(Group::int(91, i64::from(count)));
    }
    groups.push(Group::int(280, i64::from(class.was_proxy)));
    groups.push(Group::int(281, i64::from(class.is_entity)));
    groups.extend(class.extras.iter().cloned());
    PreservedRecord {
        kind: CLASS.to_owned(),
        groups,
    }
}

/// The class a record holds; `None` when a typed group has a value of the wrong type or range.
#[must_use]
pub fn class_from_record(record: &PreservedRecord) -> Option<Class> {
    let mut class = Class::default();
    let mut typed = BTreeSet::new();
    for g in &record.groups {
        let first = typed.insert(g.code);
        match (g.code, &g.value) {
            (1, Value::Text(t)) if first => class.dxf_name = t.clone(),
            (2, Value::Text(t)) if first => class.class_name = t.clone(),
            (3, Value::Text(t)) if first => class.application = t.clone(),
            (90, Value::Int(v)) if first => class.proxy_flags = i32::try_from(*v).ok()?,
            (91, Value::Int(v)) if first => class.instance_count = Some(i32::try_from(*v).ok()?),
            (280, Value::Int(v)) if first => class.was_proxy = i16::try_from(*v).ok()?,
            (281, Value::Int(v)) if first => class.is_entity = i16::try_from(*v).ok()?,
            _ => class.extras.push(g.clone()),
        }
    }
    Some(class)
}

/// The record of an APPID entry: its handle, name, flags, the groups it does not type and its
/// extended data.
#[must_use]
pub fn appid_record(entry: &TableEntry<AppId>) -> PreservedRecord {
    let mut groups = Vec::new();
    if let Some(handle) = entry.head.handle {
        groups.push(Group::text(5, handle.to_hex()));
    }
    groups.push(Group::text(2, entry.name.clone()));
    groups.push(Group::int(70, i64::from(entry.flags)));
    groups.extend(entry.extras.iter().cloned());
    groups.extend(xdata_groups(&entry.xdata));
    PreservedRecord {
        kind: APPID.to_owned(),
        groups,
    }
}

/// The APPID entry a record holds; `None` when its name or flags are missing or malformed.
#[must_use]
pub fn appid_from_record(record: &PreservedRecord) -> Option<TableEntry<AppId>> {
    let split = record
        .groups
        .iter()
        .position(|g| g.code == 1001)
        .unwrap_or(record.groups.len());
    let (body, tail) = record.groups.split_at(split);
    let mut entry = TableEntry::<AppId>::default();
    let mut named = false;
    let mut flagged = false;
    for g in body {
        match (g.code, &g.value) {
            (5, _) if entry.head.handle.is_none() => entry.head.handle = g.as_handle(),
            (2, Value::Text(t)) if !named => {
                entry.name = t.clone();
                named = true;
            }
            (70, Value::Int(v)) if !flagged => {
                entry.flags = i16::try_from(*v).ok()?;
                flagged = true;
            }
            _ => entry.extras.push(g.clone()),
        }
    }
    if !named {
        return None;
    }
    entry.xdata = xdata_from_groups(tail)?;
    Some(entry)
}

/// The record of a dictionary: its handle and owner, its flags, its entries (name and handle
/// with the hard or soft owner code) and the groups it does not type, then its extended data.
#[must_use]
pub fn dictionary_record(object: &Object, d: &Dictionary) -> PreservedRecord {
    let mut groups = Vec::new();
    if let Some(handle) = object.head.handle {
        groups.push(Group::text(5, handle.to_hex()));
    }
    if let Some(owner) = object.head.owner {
        groups.push(Group::text(330, owner.to_hex()));
    }
    if let Some(flag) = d.hard_owner {
        groups.push(Group::int(280, i64::from(flag)));
    }
    if let Some(flag) = d.cloning {
        groups.push(Group::int(281, i64::from(flag)));
    }
    for entry in &d.entries {
        groups.push(Group::text(3, entry.name.clone()));
        let code = if entry.hard_owner { 360 } else { 350 };
        groups.push(Group::text(code, entry.handle.to_hex()));
    }
    groups.extend(object.extras.iter().cloned());
    groups.extend(xdata_groups(&object.xdata));
    PreservedRecord {
        kind: DICTIONARY.to_owned(),
        groups,
    }
}

/// The dictionary object a record holds; `None` when an entry has no name or no handle, or the
/// extended data is malformed. Groups that are not typed are kept as extras in their order.
#[must_use]
pub fn dictionary_object(record: &PreservedRecord) -> Option<Object> {
    let split = record
        .groups
        .iter()
        .position(|g| g.code == 1001)
        .unwrap_or(record.groups.len());
    let (body, tail) = record.groups.split_at(split);
    let mut head = Head::default();
    let mut d = Dictionary::default();
    let mut extras = Vec::new();
    let mut started = false;
    let mut pending = false;
    for g in body {
        match (g.code, &g.value) {
            (5, _) if head.handle.is_none() => head.handle = g.as_handle(),
            (330, _) if head.owner.is_none() && !started => head.owner = g.as_handle(),
            (280, Value::Int(v)) if !started && d.hard_owner.is_none() => {
                d.hard_owner = Some(i16::try_from(*v).ok()?);
            }
            (281, Value::Int(v)) if !started && d.cloning.is_none() => {
                d.cloning = Some(i16::try_from(*v).ok()?);
            }
            (3, Value::Text(name)) if !pending => {
                started = true;
                pending = true;
                d.entries.push(DictionaryEntry {
                    name: name.clone(),
                    ..DictionaryEntry::default()
                });
            }
            (350 | 360, _) if pending => {
                let entry = d.entries.last_mut()?;
                entry.handle = g.as_handle()?;
                entry.hard_owner = g.code == 360;
                pending = false;
            }
            _ => extras.push(g.clone()),
        }
    }
    if pending {
        return None;
    }
    Some(Object {
        head,
        kind: ObjectKind::Dictionary(d),
        extras,
        xdata: xdata_from_groups(tail)?,
    })
}

/// The longest binary chunk (group 1004) the text form of extended data holds, in bytes.
pub const TEXT_CHUNK_BYTES: usize = 127;

/// The longest binary chunk the binary form holds, in bytes (the writer's limit).
pub const BINARY_CHUNK_BYTES: usize = 255;

/// Whether a name may name a registered application or another table entry: not empty, at most
/// `NAME_BYTES` bytes, and free of control characters (the rule the import applies to names).
#[must_use]
pub fn valid_name(name: &str) -> bool {
    let limit = usize::try_from(arc_draft::types::NAME_BYTES).unwrap_or(usize::MAX);
    !name.is_empty() && name.len() <= limit && crate::map_tables::clean_line(name) == name
}

/// Whether extended data is well formed: its control braces (group 1002) balance and never close
/// before they open, its binary chunks fit the output form, and every application name (group
/// 1001) is a valid table name.
#[must_use]
pub fn xdata_well_formed(groups: &[Group], binary: bool) -> bool {
    let limit = if binary {
        BINARY_CHUNK_BYTES
    } else {
        TEXT_CHUNK_BYTES
    };
    let mut depth = 0_i64;
    for g in groups {
        match (g.code, &g.value) {
            (1001, Value::Text(name)) if !valid_name(name) => return false,
            (1002, Value::Text(text)) => {
                depth += if text.starts_with('{') { 1 } else { -1 };
                if depth < 0 {
                    return false;
                }
            }
            (1004, Value::Bytes(bytes)) if bytes.len() > limit => return false,
            _ => {}
        }
    }
    depth == 0
}

/// Whether a dictionary object is a root (the named-object dictionary): it has no owner, or the
/// owner handle zero.
#[must_use]
pub fn is_root(object: &Object) -> bool {
    object.head.owner.is_none_or(|h| h.0 == 0)
}

/// Whether the group code holds an object handle of the format (soft and hard pointers and
/// owners, the reactor and dictionary codes, and the plot style and visual style handles).
#[must_use]
pub fn is_handle_code(code: i16) -> bool {
    matches!(code, 320..=369 | 390..=399 | 480 | 481)
}

/// The index of the owner group of a record: its first group 330 outside a `{...}` application
/// group and before the first subclass marker (group 100). `None` when there is none.
#[must_use]
pub fn owner_position(groups: &[Group]) -> Option<usize> {
    let mut braces = 0_u32;
    for (i, g) in groups.iter().enumerate() {
        match g.code {
            100 => return None,
            102 => {
                if g.as_text().is_some_and(|t| t.starts_with('{')) {
                    braces += 1;
                } else {
                    braces = braces.saturating_sub(1);
                }
            }
            330 if braces == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdata_round_trips_and_needs_a_leading_application() {
        let xdata = vec![
            XData {
                app: "APP_A".to_owned(),
                groups: vec![
                    Group::text(1000, "x"),
                    Group::text(1002, "{"),
                    Group::text(1005, "1F"),
                    Group::text(1002, "}"),
                ],
            },
            XData {
                app: "APP_B".to_owned(),
                groups: vec![Group::double(1040, 2.5)],
            },
        ];
        let groups = xdata_groups(&xdata);
        assert_eq!(xdata_from_groups(&groups), Some(xdata));
        assert_eq!(xdata_from_groups(&[Group::text(1000, "x")]), None);
    }

    #[test]
    fn owner_is_the_first_handle_outside_braces_before_the_markers() {
        let groups = vec![
            Group::text(5, "10"),
            Group::text(102, "{ACAD_REACTORS"),
            Group::text(330, "11"),
            Group::text(102, "}"),
            Group::text(330, "12"),
            Group::text(100, "AcDbEntity"),
            Group::text(330, "13"),
        ];
        assert_eq!(owner_position(&groups), Some(4));
        assert_eq!(owner_position(&groups[..2]), None);
    }
}
