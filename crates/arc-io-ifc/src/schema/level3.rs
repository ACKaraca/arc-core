//! Conformance level 3: the domain (`WHERE`) rules of entities and defined types and the global
//! rules, evaluated by the rule interpreter over the instances of a file.
//!
//! A domain rule is violated only when it evaluates to `FALSE` (ISO 10303-11): `TRUE` passes and
//! `UNKNOWN`, which `?` also gives, passes and is counted apart. A rule the interpreter stops on
//! (an unsupported operand, a bound of the caller's limits, an instance whose structure is wrong) is
//! not evaluated and never passes.

use std::collections::HashMap;

use arc_base::{Cancel, Limits, Result};

use crate::error::rules::{self, Rule};
use crate::model::{IfcFile, Instance};

use super::eval::{Inst, Interp, Stop, Value};
use super::inverse::InverseIndex;
use super::tree::Logical;
use super::{Named, Position, SchemaTables, TypeRef};

/// What a domain rule belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleScope {
    /// An entity; the rule applies to every instance of the entity and its subtypes.
    Entity,
    /// A defined type; the rule applies to every attribute value of the type or a type based on
    /// it.
    Type,
    /// A global rule; it applies once to the file.
    Global,
}

/// The outcome of one domain rule over a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhereCount {
    /// What the rule belongs to.
    pub scope: RuleScope,
    /// The entity, defined type or global rule as the schema writes it.
    pub owner: &'static str,
    /// The label, such as `WR1`; empty when the schema gives none.
    pub label: &'static str,
    /// Instances (for a global rule, the file) the rule is `TRUE` for.
    pub passed: u64,
    /// Instances it is `UNKNOWN` for; the rule holds.
    pub unknown: u64,
    /// Instances it is `FALSE` for.
    pub failed: u64,
    /// Instances it could not be evaluated for.
    pub not_evaluated: u64,
    /// The reason of the first evaluation that stopped.
    pub reason: Option<String>,
}

impl WhereCount {
    /// `owner.label`, the name findings give the rule.
    #[must_use]
    pub fn name(&self) -> String {
        format!("{}.{}", self.owner, self.label)
    }
}

/// Outcome of one rule on one instance, ordered so that the strongest one wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    None,
    Passed,
    NotEvaluated,
    Failed,
}

/// The result of level 3.
pub(super) struct Level3 {
    pub counts: Vec<WhereCount>,
    /// Per rule class (entity, type, global): passed, failed, not evaluated.
    pub classes: [(Rule, [u64; 3]); 3],
    /// Failed checks: instance, entity as written, rule name and class.
    pub findings: Vec<(u64, String, String, Rule)>,
}

/// The first count of each entity's and defined type's rules in [`Level3::counts`].
struct Slots {
    entity: Vec<usize>,
    defined: Vec<usize>,
    global: Vec<usize>,
}

fn counts(tables: &SchemaTables) -> (Vec<WhereCount>, Slots) {
    let mut out = Vec::new();
    let mut slots = Slots {
        entity: Vec::new(),
        defined: Vec::new(),
        global: Vec::new(),
    };
    let add = |out: &mut Vec<WhereCount>, scope, owner, rules: &'static [super::WhereRule]| {
        let first = out.len();
        out.extend(rules.iter().map(|r| WhereCount {
            scope,
            owner,
            label: r.label.unwrap_or_default(),
            passed: 0,
            unknown: 0,
            failed: 0,
            not_evaluated: 0,
            reason: None,
        }));
        first
    };
    for e in tables.entities {
        slots
            .entity
            .push(add(&mut out, RuleScope::Entity, e.name, e.where_rules));
    }
    for d in tables.defined_types {
        slots
            .defined
            .push(add(&mut out, RuleScope::Type, d.name, d.where_rules));
    }
    for r in tables.rules {
        slots
            .global
            .push(add(&mut out, RuleScope::Global, r.name, r.where_rules));
    }
    (out, slots)
}

/// Records an outcome in a count and returns the instance state.
fn record(count: &mut WhereCount, outcome: std::result::Result<Logical, String>) -> State {
    match outcome {
        Ok(Logical::True) => {
            count.passed += 1;
            State::Passed
        }
        Ok(Logical::Unknown) => {
            count.unknown += 1;
            State::Passed
        }
        Ok(Logical::False) => {
            count.failed += 1;
            State::Failed
        }
        Err(reason) => {
            count.not_evaluated += 1;
            count.reason.get_or_insert(reason);
            State::NotEvaluated
        }
    }
}

/// Splits a cancellation from the outcome of a rule.
fn outcome(r: std::result::Result<Logical, Stop>) -> Result<std::result::Result<Logical, String>> {
    match r {
        Ok(l) => Ok(Ok(l)),
        Err(Stop::NotEvaluated(reason)) => Ok(Err(reason)),
        Err(Stop::Cancelled(e)) => Err(e),
    }
}

const ENTITY: usize = 0;
const TYPE: usize = 1;
const GLOBAL: usize = 2;

/// Evaluates every domain and global rule of the file's schema.
#[allow(
    clippy::too_many_lines,
    reason = "the three scopes of rules in one pass"
)]
pub(super) fn check(
    file: &IfcFile,
    tables: &'static SchemaTables,
    index: &InverseIndex,
    limits: &Limits,
    cancel: &Cancel,
) -> Result<Level3> {
    let (mut counts, slots) = counts(tables);
    let mut classes = [
        (rules::WHERE_RULE, [0u64; 3]),
        (rules::TYPE_RULE, [0u64; 3]),
        (rules::GLOBAL_RULE, [0u64; 3]),
    ];
    let mut findings = Vec::new();
    let mut interp = Interp::new(file, tables, index, limits, cancel);
    let mut applicable: HashMap<u16, Vec<(u16, usize)>> = HashMap::new();
    let mut states: HashMap<usize, State> = HashMap::new();
    let mut positions: HashMap<u16, Vec<Position>> = HashMap::new();

    for instance in file.instances() {
        cancel.check()?;
        let parts: Option<Vec<u16>> = instance
            .records
            .iter()
            .map(|r| tables.entity_index(&r.type_name))
            .collect();
        // An unknown entity has no rules; level 1 reports it.
        let Some(parts) = parts else { continue };
        let simple: &[Position] = match parts.as_slice() {
            [entity] if instance.records.len() == 1 => positions
                .entry(*entity)
                .or_insert_with(|| tables.positions(*entity)),
            _ => &[],
        };
        let sound = structure_sound(tables, instance, &parts, simple);
        let entity_name = || {
            instance
                .records
                .iter()
                .map(|r| r.type_name.as_str())
                .collect::<Vec<_>>()
                .join("+")
        };

        // Entity rules of every part and its supertypes, each once.
        let mut rules_here: Vec<(u16, usize)> = Vec::new();
        for &part in &parts {
            let list = applicable.entry(part).or_insert_with(|| {
                tables
                    .supertypes(part)
                    .flat_map(|e| {
                        let n = tables
                            .entities
                            .get(usize::from(e))
                            .map_or(0, |d| d.where_rules.len());
                        (0..n).map(move |i| (e, i))
                    })
                    .collect()
            });
            for r in list.iter() {
                if !rules_here.contains(r) {
                    rules_here.push(*r);
                }
            }
        }
        let mut class_state = State::None;
        for (owner, i) in rules_here {
            let (Some(slot), Some(rule)) = (
                slots.entity.get(usize::from(owner)).map(|s| s + i),
                tables
                    .entities
                    .get(usize::from(owner))
                    .and_then(|e| e.where_rules.get(i)),
            ) else {
                continue;
            };
            let result = if sound {
                outcome(interp.rule(&rule.tree, Value::Entity(Inst::File(instance.name))))?
            } else {
                Err("the instance's structure does not conform (level 1)".to_owned())
            };
            let Some(count) = counts.get_mut(slot) else {
                continue;
            };
            let state = record(count, result);
            if state == State::Failed {
                findings.push((
                    instance.name,
                    entity_name(),
                    count.name(),
                    rules::WHERE_RULE,
                ));
            }
            class_state = class_state.max(state);
        }
        tally(&mut classes[ENTITY].1, class_state);

        // Rules of the defined types of the attribute values.
        states.clear();
        if sound {
            let mut values = Vec::new();
            for (ty, parameter) in explicit_values(tables, instance, &parts, simple) {
                match interp.read(parameter, Some(ty)) {
                    Ok(v) => values.push(v),
                    Err(Stop::Cancelled(e)) => return Err(e),
                    Err(Stop::NotEvaluated(_)) => {}
                }
            }
            let mut pending = values;
            while let Some(value) = pending.pop() {
                match &value {
                    Value::Typed(d, inner) => {
                        for defined in based_on(tables, *d) {
                            let Some(def) = tables.defined_types.get(usize::from(defined)) else {
                                continue;
                            };
                            for (i, rule) in def.where_rules.iter().enumerate() {
                                let Some(slot) = slots.defined.get(usize::from(defined)) else {
                                    continue;
                                };
                                let result = outcome(interp.rule(&rule.tree, value.clone()))?;
                                let Some(count) = counts.get_mut(slot + i) else {
                                    continue;
                                };
                                let state = record(count, result);
                                let entry = states.entry(slot + i).or_insert(State::None);
                                if state == State::Failed && *entry != State::Failed {
                                    findings.push((
                                        instance.name,
                                        entity_name(),
                                        count.name(),
                                        rules::TYPE_RULE,
                                    ));
                                }
                                *entry = (*entry).max(state);
                            }
                        }
                        pending.push(Value::clone(inner));
                    }
                    Value::Aggregate(a) => pending.extend(a.items.iter().cloned()),
                    _ => {}
                }
            }
        }
        let type_state = states.values().copied().max().unwrap_or(State::None);
        tally(&mut classes[TYPE].1, type_state);
    }

    for (r, rule) in tables.rules.iter().enumerate() {
        cancel.check()?;
        let Some(&first) = slots.global.get(r) else {
            continue;
        };
        for (i, result) in interp
            .global(&rule.tree, rule.where_rules)
            .into_iter()
            .enumerate()
        {
            let result = outcome(result)?;
            let Some(count) = counts.get_mut(first + i) else {
                continue;
            };
            let state = record(count, result);
            if state == State::Failed {
                findings.push((0, String::new(), count.name(), rules::GLOBAL_RULE));
            }
            tally(&mut classes[GLOBAL].1, state);
        }
    }
    Ok(Level3 {
        counts,
        classes,
        findings,
    })
}

fn tally(class: &mut [u64; 3], state: State) {
    match state {
        State::None => {}
        State::Passed => class[0] += 1,
        State::Failed => class[1] += 1,
        State::NotEvaluated => class[2] += 1,
    }
}

/// The defined type and the defined types it is based on, nearest first.
fn based_on(tables: &SchemaTables, d: u16) -> Vec<u16> {
    let mut out = vec![d];
    let mut current = d;
    for _ in 0..32 {
        match tables
            .defined_types
            .get(usize::from(current))
            .map(|t| t.underlying)
        {
            Some(TypeRef::Named(Named::Defined(next))) if !out.contains(&next) => {
                out.push(next);
                current = next;
            }
            _ => break,
        }
    }
    out
}

/// Whether the records hold as many parameters as the entity declares, so that attributes are
/// found at their positions.
fn structure_sound(
    tables: &SchemaTables,
    instance: &Instance,
    parts: &[u16],
    simple: &[Position],
) -> bool {
    match (instance.records.as_slice(), parts) {
        ([record], [_]) => simple.len() == record.parameters.len(),
        _ => instance.records.iter().zip(parts).all(|(record, &part)| {
            tables
                .entities
                .get(usize::from(part))
                .is_some_and(|e| e.attributes.len() == record.parameters.len())
        }),
    }
}

/// The explicit attribute values of an instance that are not derived, with their declared types.
fn explicit_values<'a>(
    tables: &SchemaTables,
    instance: &'a Instance,
    parts: &[u16],
    simple: &[Position],
) -> Vec<(TypeRef, &'a crate::model::Parameter)> {
    let mut out = Vec::new();
    match (instance.records.as_slice(), parts) {
        ([record], [_]) => {
            for (position, parameter) in simple.iter().zip(&record.parameters) {
                if !position.derived {
                    out.push((position.attribute.ty, parameter));
                }
            }
        }
        _ => {
            for (record, &part) in instance.records.iter().zip(parts) {
                if let Some(e) = tables.entities.get(usize::from(part)) {
                    for (a, parameter) in e.attributes.iter().zip(&record.parameters) {
                        out.push((a.ty, parameter));
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::super::eval::{Agg, Built};
    use super::*;
    use crate::model::Schema;
    use crate::{ReadOptions, read};

    /// A value of the underlying type of a defined type, for its rules.
    fn sample(tables: &SchemaTables, ty: TypeRef) -> Value {
        match tables.underlying(ty) {
            TypeRef::Integer => Value::Integer(1),
            TypeRef::Real | TypeRef::Number => Value::Real(0.5),
            TypeRef::Boolean | TypeRef::Logical => Value::Logical(Logical::True),
            TypeRef::String { .. } => Value::String(Rc::from("a")),
            TypeRef::Binary { .. } => Value::Binary(Rc::from("1")),
            TypeRef::Aggregate(a) => Value::Aggregate(Rc::new(Agg {
                kind: a.kind,
                lower: 1,
                items: vec![sample(tables, a.element); 3],
            })),
            TypeRef::Named(_) => Value::Indeterminate,
        }
    }

    /// Every rule of the three schemas evaluates without stopping on a value whose attributes are
    /// all `?` (entity rules), a value of its type (type rules) and an empty file (global rules):
    /// no rule uses a construct the interpreter does not support on these paths.
    #[test]
    fn every_rule_evaluates_on_indeterminate_values() {
        for schema in [Schema::Ifc2x3, Schema::Ifc4, Schema::Ifc4x3] {
            let Some(tables) = SchemaTables::of(&schema) else {
                return;
            };
            let text = format!(
                "ISO-10303-21;HEADER;FILE_DESCRIPTION((''),'2;1');\
                 FILE_NAME('','',(''),(''),'','','');FILE_SCHEMA(('{}'));ENDSEC;DATA;ENDSEC;\
                 END-ISO-10303-21;",
                schema.identifier()
            );
            let file = read(text.as_bytes(), &mut ReadOptions::default())
                .unwrap()
                .file;
            let index = InverseIndex::default();
            let cancel = Cancel::new();
            let mut interp = Interp::new(&file, tables, &index, &Limits::DEFAULT, &cancel);
            let mut stopped = Vec::new();
            let mut total = 0;
            for (i, e) in tables.entities.iter().enumerate() {
                let i = u16::try_from(i).unwrap();
                let parts = tables
                    .supertypes(i)
                    .map(|p| {
                        let n = tables.entities[usize::from(p)].attributes.len();
                        (p, vec![Value::Indeterminate; n])
                    })
                    .collect();
                let this = Value::Entity(Inst::Built(Rc::new(Built { parts })));
                for r in e.where_rules {
                    total += 1;
                    if let Err(Stop::NotEvaluated(why)) = interp.rule(&r.tree, this.clone()) {
                        stopped.push(format!("{}.{:?}: {why}", e.name, r.label));
                    }
                }
            }
            for (d, t) in tables.defined_types.iter().enumerate() {
                let d = u16::try_from(d).unwrap();
                let value = Value::Typed(d, Rc::new(sample(tables, t.underlying)));
                for r in t.where_rules {
                    total += 1;
                    if let Err(Stop::NotEvaluated(why)) = interp.rule(&r.tree, value.clone()) {
                        stopped.push(format!("{}.{:?}: {why}", t.name, r.label));
                    }
                }
            }
            for rule in tables.rules {
                for (r, result) in rule
                    .where_rules
                    .iter()
                    .zip(interp.global(&rule.tree, rule.where_rules))
                {
                    total += 1;
                    if let Err(Stop::NotEvaluated(why)) = result {
                        stopped.push(format!("{}.{:?}: {why}", rule.name, r.label));
                    }
                }
            }
            assert!(
                stopped.is_empty(),
                "{}: {total} rules, {stopped:#?}",
                tables.name
            );
        }
    }
}
