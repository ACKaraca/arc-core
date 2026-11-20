//! Header variables: the import's checks and unit conversion, and the checks the export applies
//! before it writes a stored variable back. A variable the export would write stale, dangling or
//! in a form the writer refuses is omitted and reported; no other variable is changed.
//!
//! Names are compared without regard to case everywhere; a name is kept as the file had it.

use std::collections::{BTreeMap, BTreeSet};

use arc_io_dxf::loss::Argument;
use arc_io_dxf::model::HeaderVariable;
use arc_io_dxf::{Drawing, Group, Value, ValueKind, value_kind, value_writable};

use crate::error::reasons;
use crate::report::Reporter;

/// The variables the export derives from the drawing's content and the editing history, or that
/// depend on the clock and the session. The export computes none of them, so a stored value would
/// be stale: each is omitted.
pub(crate) const DERIVED: &[&str] = &[
    "$EXTMIN",
    "$EXTMAX",
    "$PEXTMIN",
    "$PEXTMAX",
    "$TDUPDATE",
    "$TDUUPDATE",
    "$TDINDWG",
    "$TDUSRTIMER",
];

/// The code page variable. The export writes UTF-8 (version R2018) and the writer uses the code
/// page only for versions before 2007, so the stored page would name an encoding that is not used.
pub(crate) const CODE_PAGE: &str = "$DWGCODEPAGE";

/// The most names one loss item lists; the item's count is always the full number.
const MAX_NAMES: usize = 16;

/// Why a stored header variable is not written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Omission {
    /// A name that is not a variable name, a value that is not finite, does not fit its group
    /// code, is not writable in the output form, or a point that is incomplete; or the code page.
    Value,
    /// A variable derived from the drawing's content ([`DERIVED`]), or one that the unit
    /// conversion made stale.
    Derived,
    /// A handle that names no object the export writes under that handle.
    Handle,
    /// A name that names no entry of the table the variable refers to.
    Name,
}

impl Omission {
    fn reason(self) -> arc_io_dxf::loss::Reason {
        match self {
            Self::Value => reasons::HEADER_VALUE_OMITTED,
            Self::Derived => reasons::HEADER_DERIVED_OMITTED,
            Self::Handle => reasons::HEADER_HANDLE_OMITTED,
            Self::Name => reasons::HEADER_NAME_OMITTED,
        }
    }
}

/// The tables a name-valued header variable names an entry of.
#[derive(Debug, Clone, Copy)]
enum Table {
    Layers,
    Linetypes,
    Styles,
    DimStyles,
    Blocks,
    /// The UCS names. The export writes no UCS table: only the empty name, which is the world
    /// coordinate system, is written.
    Ucs,
    /// A table the export does not write; no name of it is written.
    Unwritten,
}

fn table_of(name: &str) -> Option<Table> {
    Some(match name.to_ascii_uppercase().as_str() {
        "$CLAYER" => Table::Layers,
        "$CELTYPE" | "$DIMLTYPE" | "$DIMLTEX1" | "$DIMLTEX2" => Table::Linetypes,
        "$TEXTSTYLE" | "$DIMTXSTY" => Table::Styles,
        "$DIMSTYLE" => Table::DimStyles,
        "$DIMBLK" | "$DIMBLK1" | "$DIMBLK2" | "$DIMLDRBLK" => Table::Blocks,
        "$UCSNAME" | "$UCSBASE" | "$PUCSNAME" | "$PUCSBASE" => Table::Ucs,
        "$CMLSTYLE" => Table::Unwritten,
        _ => return None,
    })
}

/// Whether a group code carries a handle: 5, 105, 320 to 369, 390 to 399, 480 and 481.
fn is_handle(code: i16) -> bool {
    matches!(code, 5 | 105 | 320..=369 | 390..=399 | 480 | 481)
}

/// Whether `name` is a variable name: a dollar sign, then letters, digits or underscores.
#[must_use]
pub(crate) fn valid_name(name: &str) -> bool {
    name.strip_prefix('$').is_some_and(|rest| {
        !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    })
}

/// Whether `values` are non-empty, every value has the kind its group code gives, every real is
/// finite, and every point is complete: an X group (10 to 18) is followed by its Y group, a Y group (20 to 28)
/// follows its X, and a Z group (30 to 38) follows its Y. A code the crate does not know takes
/// text, as the reader keeps it.
fn values_are_complete(values: &[Group]) -> bool {
    !values.is_empty()
        && values.iter().all(|g| match (&g.value, value_kind(g.code)) {
            (Value::Double(v), Some(ValueKind::Double)) => v.is_finite(),
            (
                Value::Int(_),
                Some(ValueKind::Int8 | ValueKind::Int16 | ValueKind::Int32 | ValueKind::Int64),
            ) => true,
            (Value::Bool(_), Some(ValueKind::Bool)) => true,
            (Value::Bytes(_), Some(ValueKind::Bytes)) => true,
            (Value::Text(_), Some(ValueKind::Text) | None) => true,
            _ => false,
        })
        && values.iter().enumerate().all(|(i, g)| {
            let before = i.checked_sub(1).map(|p| values[p].code);
            let after = values.get(i + 1).map(|n| n.code);
            match g.code {
                10..=18 => after == Some(g.code + 10),
                20..=28 | 30..=38 => before == Some(g.code - 10),
                _ => true,
            }
        })
}

/// Whether the variable can be stored: a valid name and complete values of their kinds.
#[must_use]
pub(crate) fn storable(var: &HeaderVariable) -> bool {
    valid_name(&var.name) && values_are_complete(&var.values)
}

/// The lowercase names of the entries of `table` in the drawing the export writes.
fn names_of(drawing: &Drawing, table: Table) -> BTreeSet<String> {
    fn of<T>(table: &Option<arc_io_dxf::model::Table<T>>) -> Vec<String> {
        table
            .iter()
            .flat_map(|t| t.entries.iter().map(|e| e.name.to_lowercase()))
            .collect()
    }
    let t = &drawing.tables;
    let names = match table {
        Table::Layers => of(&t.layers),
        Table::Linetypes => of(&t.linetypes),
        Table::Styles => of(&t.styles),
        Table::DimStyles => of(&t.dimstyles),
        Table::Blocks => of(&t.block_records),
        Table::Ucs | Table::Unwritten => Vec::new(),
    };
    names.into_iter().collect()
}

/// Why the export does not write `var` back, when it does not. `drawing` is the drawing the export
/// writes; `kept` holds the handles of the objects it writes under their bound handles; `binary`
/// is the output form the writer is given.
#[must_use]
pub(crate) fn export_omission(
    var: &HeaderVariable,
    drawing: &Drawing,
    kept: &BTreeSet<u64>,
    binary: bool,
) -> Option<Omission> {
    let writable = var.values.iter().all(|g| value_writable(g, binary));
    if !storable(var) || !writable || var.name.eq_ignore_ascii_case(CODE_PAGE) {
        return Some(Omission::Value);
    }
    if DERIVED.iter().any(|d| d.eq_ignore_ascii_case(&var.name)) {
        return Some(Omission::Derived);
    }
    if let Some(table) = table_of(&var.name) {
        let text = var
            .values
            .iter()
            .find_map(Group::as_text)
            .unwrap_or_default();
        // An empty block name names no block (the default arrow), and an empty UCS name is the
        // world coordinate system.
        let named = match table {
            Table::Blocks | Table::Ucs if text.is_empty() => true,
            Table::Unwritten => false,
            _ => names_of(drawing, table).contains(&text.to_lowercase()),
        };
        if !named {
            return Some(Omission::Name);
        }
    }
    let handles = var.values.iter().filter(|g| is_handle(g.code));
    if !handles
        .clone()
        .all(|g| g.as_handle().is_some_and(|h| kept.contains(&h.0)))
    {
        return Some(Omission::Handle);
    }
    None
}

/// Adds `var` to `out` at the place of the variable of its name, or at the end. A replacing
/// variable of the export takes its own spelling of the name; a stored one keeps the first.
fn put(
    out: &mut Vec<HeaderVariable>,
    at: &mut BTreeMap<String, usize>,
    var: HeaderVariable,
    own_spelling: bool,
) {
    let key = var.name.to_ascii_uppercase();
    if let Some(slot) = at.get(&key).and_then(|&i| out.get_mut(i)) {
        if own_spelling {
            slot.name = var.name;
        }
        slot.values = var.values;
    } else {
        at.insert(key, out.len());
        out.push(var);
    }
}

/// Reports the names of the omitted variables under `reason`, one loss item per reason.
pub(crate) fn report(rep: &mut Reporter, reason: arc_io_dxf::loss::Reason, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let shown: Vec<&str> = names.iter().take(MAX_NAMES).map(String::as_str).collect();
    let count = u64::try_from(names.len()).unwrap_or(u64::MAX);
    rep.note_n(
        reason,
        "HEADER",
        None,
        count,
        &[("names", Argument::Text(shown.join(", ")))],
    );
}

/// The write-back of an export. `stored` are the stored variables in their order; `owned` are the
/// variables the export writes itself, which take the place of a stored variable of their name
/// (without regard to case) or follow the stored ones. Stored variables that the checks omit are
/// reported and not written.
#[must_use]
pub(crate) fn write_back(
    stored: Vec<HeaderVariable>,
    owned: Vec<HeaderVariable>,
    drawing: &Drawing,
    kept: &BTreeSet<u64>,
    binary: bool,
    rep: &mut Reporter,
) -> Vec<HeaderVariable> {
    let owned_names: BTreeSet<String> = owned.iter().map(|v| v.name.to_ascii_uppercase()).collect();
    let mut out = Vec::new();
    let mut at = BTreeMap::new();
    let mut omitted: BTreeMap<Omission, Vec<String>> = BTreeMap::new();
    for var in stored {
        if !owned_names.contains(&var.name.to_ascii_uppercase())
            && let Some(omission) = export_omission(&var, drawing, kept, binary)
        {
            omitted.entry(omission).or_default().push(var.name);
            continue;
        }
        put(&mut out, &mut at, var, false);
    }
    for var in owned {
        put(&mut out, &mut at, var, true);
    }
    for (omission, names) in omitted {
        report(rep, omission.reason(), &names);
    }
    out
}

/// The unit class of a header variable of known meaning. Lengths and points are in the drawing's
/// units and change with a unit conversion; the rest do not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A length (group 40).
    Length,
    /// A length that is a size only when positive; a negative one is a percentage of the view.
    PositiveLength,
    /// A point: its X, Y and Z groups (codes 10 to 39) are lengths.
    Point,
    /// A ratio, an angle, a direction, a flag, a time or a count: unchanged by a conversion.
    Other,
}

fn class_of(name: &str) -> Option<Class> {
    Some(match name.to_ascii_uppercase().as_str() {
        "$TEXTSIZE" | "$ELEVATION" | "$PELEVATION" | "$THICKNESS" | "$TRACEWID" | "$VIEWSIZE"
        | "$FILLETRAD" | "$CHAMFERA" | "$CHAMFERB" | "$CHAMFERC" | "$PLINEWID" | "$DIMASZ"
        | "$DIMTXT" | "$DIMEXO" | "$DIMEXE" | "$DIMGAP" | "$DIMDLI" | "$DIMDLE" | "$DIMTSZ"
        | "$DIMCEN" | "$DIMRND" | "$DIMTP" | "$DIMTM" | "$DIMFXL" => Class::Length,
        "$PDSIZE" => Class::PositiveLength,
        "$LIMMIN" | "$LIMMAX" | "$PLIMMIN" | "$PLIMMAX" | "$INSBASE" | "$PINSBASE" | "$VIEWCTR"
        | "$SNAPBASE" | "$SNAPUNIT" | "$GRIDUNIT" | "$UCSORG" | "$PUCSORG" => Class::Point,
        "$LTSCALE" | "$CELTSCALE" | "$DIMSCALE" | "$DIMLFAC" | "$DIMTFAC" | "$DIMALTF"
        | "$DIMTVP" | "$CMLSCALE" | "$FACETRES" | "$PSVPSCALE" | "$TSTACKSIZE" | "$ANGBASE"
        | "$DIMJOGANG" | "$SNAPANG" | "$LOFTANG1" | "$LOFTANG2" | "$CHAMFERD" | "$VIEWTWIST"
        | "$TDCREATE" | "$UCSXDIR" | "$UCSYDIR" | "$PUCSXDIR" | "$PUCSYDIR" | "$LENSLENGTH" => {
            Class::Other
        }
        _ => return None,
    })
}

/// Whether `var` holds a real but its name has no known unit class: a conversion keeps it as it
/// is and says so.
fn unknown_real(var: &HeaderVariable) -> bool {
    class_of(&var.name).is_none()
        && var
            .values
            .iter()
            .any(|g| matches!(g.value, Value::Double(_)))
}

/// Multiplies the lengths of `var` by `factor`, as its class says.
fn scale(var: &mut HeaderVariable, factor: f64) {
    let Some(class) = class_of(&var.name) else {
        return;
    };
    for g in &mut var.values {
        let code = g.code;
        let Value::Double(v) = &mut g.value else {
            continue;
        };
        let length = match class {
            Class::Length => code == 40,
            Class::PositiveLength => code == 40 && *v > 0.0,
            Class::Point => (10..=39).contains(&code),
            Class::Other => false,
        };
        if length {
            *v *= factor;
        }
    }
}

/// The header variables a unit conversion changes, with what it does to them.
#[derive(Debug, Default)]
pub(crate) struct Conversion {
    /// The names of the variables kept without scaling, whose unit class is not known.
    pub(crate) unscaled: Vec<String>,
    /// The names of the variables dropped because the conversion made them stale.
    pub(crate) dropped: Vec<String>,
}

/// Brings the header variables kept by an import into the target's units. `vars` are the variables
/// the import keeps, in file order. `factor` is the import's unit factor; `target` is whether the
/// target unit is metric (`None` when it is unitless); `source` is whether the file's unit is
/// metric (`None` when unknown). When the file states a measurement system (its `$MEASUREMENT`, or
/// else its unit) and it is not the target's, `$MEASUREMENT` is written as the target's and the
/// linear unit format of the file is dropped. A file that states neither keeps its variables.
pub(crate) fn convert(
    vars: &mut Vec<HeaderVariable>,
    factor: f64,
    target: Option<bool>,
    source: Option<bool>,
) -> Conversion {
    let mut out = Conversion::default();
    let file_measurement = vars
        .iter()
        .find(|v| v.name.eq_ignore_ascii_case("$MEASUREMENT"))
        .and_then(|v| {
            v.values.iter().find_map(|g| match g.value {
                Value::Int(m) => Some(m != 0),
                _ => None,
            })
        });
    if let Some(metric) = target {
        // The system the file states: its $MEASUREMENT, else the system of its unit.
        let from = file_measurement.or(source);
        let differs = from.is_some_and(|m| m != metric);
        if file_measurement.is_some() || differs {
            let measurement = HeaderVariable {
                name: "$MEASUREMENT".to_owned(),
                values: vec![Group::int(70, i64::from(metric))],
            };
            match vars
                .iter()
                .position(|v| v.name.eq_ignore_ascii_case("$MEASUREMENT"))
            {
                Some(i) => vars[i] = measurement,
                None => vars.push(measurement),
            }
        }
        if differs {
            let before = vars.len();
            vars.retain(|v| !v.name.eq_ignore_ascii_case("$LUNITS"));
            if vars.len() != before {
                out.dropped.push("$LUNITS".to_owned());
            }
        }
    }
    if (factor - 1.0).abs() > f64::EPSILON {
        for var in vars.iter_mut() {
            scale(var, factor);
            if unknown_real(var) {
                out.unscaled.push(var.name.clone());
            }
        }
    }
    out
}

/// Reports the variables a conversion dropped or kept unscaled, one loss item for each reason.
pub(crate) fn report_conversion(rep: &mut Reporter, conversion: &Conversion) {
    report(rep, reasons::HEADER_DERIVED_OMITTED, &conversion.dropped);
    report(rep, reasons::HEADER_UNSCALED, &conversion.unscaled);
}
