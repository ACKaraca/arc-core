//! The conformance checker (ADR-0018): level 1, the structure of every instance against the
//! tables of the file's schema; level 2, inverse attribute cardinalities and `UNIQUE` rules;
//! level 3, the domain and global rules ([`super::level3`]).
//!
//! Every rule is counted per instance: passed, failed, or not evaluated. An instance is not
//! evaluated for a rule when the rule could not be decided (the entity is unknown, the attribute
//! count is wrong so positions are unknown, a bound is an expression, a referenced instance has an
//! unknown entity), and a rule is never counted as passed unless it was decided. Without tables
//! every rule is not evaluated for every instance.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use arc_base::{Cancel, Limits, Result};

use crate::error::limit;
use crate::error::rules::{self, Rule};
use crate::model::{IfcFile, Instance, Parameter, Record};

use super::inverse::InverseIndex;
use super::level3::{self, WhereCount};
use super::{AggregateKind, Attribute, Bound, Inverse, Named, Position, SchemaTables, TypeRef};

/// The most findings kept; the counts stay exact beyond it.
pub const MAX_FINDINGS: usize = 10_000;

/// One failed rule on one instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The instance name; 0 for a global rule.
    pub instance: u64,
    /// The entity of the instance as written, upper case; for a complex instance, its parts
    /// joined by `+`; empty for a global rule.
    pub entity: String,
    /// The attribute concerned: an explicit attribute, an inverse attribute, the attributes of
    /// a `UNIQUE` rule joined by `,`, or for level 3 the rule as `Owner.Label`.
    pub attribute: Option<String>,
    /// The rule.
    pub rule: Rule,
}

/// The outcome of one rule over all instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuleCount {
    /// The rule.
    pub rule: Rule,
    /// Instances the rule applies to and holds for.
    pub passed: u64,
    /// Instances the rule fails for.
    pub failed: u64,
    /// Instances the rule applies to but could not be decided for.
    pub not_evaluated: u64,
}

/// The result of a conformance check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conformance {
    /// The schema of the tables the file was checked against, or `None` when there are none.
    pub schema: Option<&'static str>,
    /// The findings in file order, at most [`MAX_FINDINGS`] and at most `record_count`.
    pub findings: Vec<Finding>,
    /// Whether findings were left out because of the bound.
    pub truncated: bool,
    /// One count per rule, in the order of [`rules::ALL`]. For the level 3 rules an instance
    /// passes when every applicable rule is `TRUE` or `UNKNOWN`; a global rule counts once per
    /// domain rule.
    pub rules: Vec<RuleCount>,
    /// Level 3 per domain rule of the schema: entity rules in entity order, then defined type
    /// rules, then global rules. Empty without tables.
    pub where_rules: Vec<WhereCount>,
}

impl Conformance {
    /// The count of `rule`.
    #[must_use]
    pub fn count(&self, rule: Rule) -> Option<&RuleCount> {
        self.rules.iter().find(|c| c.rule == rule)
    }

    /// The number of failed rule checks over all rules.
    #[must_use]
    pub fn failed(&self) -> u64 {
        self.rules.iter().map(|c| c.failed).sum()
    }

    /// The number of rule checks that were not evaluated over all rules.
    #[must_use]
    pub fn not_evaluated(&self) -> u64 {
        self.rules.iter().map(|c| c.not_evaluated).sum()
    }
}

/// Outcome of a rule on one instance, ordered so that the strongest one wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    NotApplicable,
    Passed,
    NotEvaluated,
    Failed,
}

/// The index of a rule in [`rules::ALL`].
fn slot(rule: Rule) -> usize {
    rules::ALL
        .iter()
        .position(|r| *r == rule)
        .unwrap_or_default()
}

/// The rules that read attribute values; not evaluated when the positions are unknown.
const VALUE_RULES: [Rule; 10] = [
    rules::VALUE_MISSING,
    rules::DERIVED_MISPLACED,
    rules::VALUE_TYPE,
    rules::ENUMERATION_ITEM,
    rules::SELECT_MEMBER,
    rules::REFERENCE_TYPE,
    rules::REFERENCE_DANGLING,
    rules::AGGREGATE_SIZE,
    rules::AGGREGATE_UNIQUE,
    rules::VALUE_WIDTH,
];

/// Bytes charged per key of a `UNIQUE` rule, besides its text.
const KEY_BYTES: u64 = 64;

impl IfcFile {
    /// Checks every instance against the tables of the file's schema: level 1 (instance
    /// structure) and level 2 (inverse attribute cardinalities and `UNIQUE` rules).
    ///
    /// The inverse index and the keys of the `UNIQUE` rules are charged against the share of
    /// `memory_bytes` left after the parser and count against `record_count`; cancellation is
    /// checked per instance.
    ///
    /// # Errors
    ///
    /// `io.limit_exceeded` when a limit is exceeded, `base.cancelled` when cancelled.
    pub fn conformance(&self, limits: &Limits, cancel: &Cancel) -> Result<Conformance> {
        let mut counts = vec![[0u64; 3]; rules::ALL.len()];
        let max_findings = usize::try_from(limits.record_count)
            .unwrap_or(usize::MAX)
            .min(MAX_FINDINGS);
        let Some(tables) = self.tables() else {
            let n = u64::try_from(self.instances().len()).unwrap_or(u64::MAX);
            return Ok(Conformance {
                schema: None,
                findings: Vec::new(),
                truncated: false,
                rules: rules::ALL
                    .iter()
                    .map(|&rule| RuleCount {
                        rule,
                        passed: 0,
                        failed: 0,
                        not_evaluated: n,
                    })
                    .collect(),
                where_rules: Vec::new(),
            });
        };
        let index = self.inverse_index(limits, cancel)?;
        let mut checker = Checker {
            file: self,
            tables,
            positions: HashMap::new(),
            states: [State::NotApplicable; RULES],
            reported: BTreeSet::new(),
            findings: Vec::new(),
            truncated: false,
            max_findings,
            instance: 0,
            entity: String::new(),
        };
        let mut unique = UniqueKeys {
            seen: BTreeMap::new(),
            used: 0,
            memory: limits
                .memory_bytes
                .map(|max| max.saturating_sub(self.parsed_bytes())),
            record_limit: limits.record_count,
        };
        unique.count(self, tables, cancel)?;
        for instance in self.instances() {
            cancel.check()?;
            checker.start(instance);
            let parts = checker.structure(instance);
            if let Some(parts) = parts {
                checker.inverses(instance, &parts, &index);
                checker.unique(instance, &parts, &unique);
            } else {
                checker.set(rules::INVERSE_CARDINALITY, State::NotEvaluated);
                checker.set(rules::UNIQUE_RULE, State::NotEvaluated);
            }
            for (count, state) in counts.iter_mut().zip(checker.states) {
                match state {
                    State::NotApplicable => {}
                    State::Passed => count[0] += 1,
                    State::Failed => count[1] += 1,
                    State::NotEvaluated => count[2] += 1,
                }
            }
        }
        let level3 = level3::check(self, tables, &index, limits, cancel)?;
        for (rule, class) in level3.classes {
            counts[slot(rule)] = class;
        }
        for (instance, entity, name, rule) in level3.findings {
            if checker.findings.len() >= max_findings {
                checker.truncated = true;
                break;
            }
            checker.findings.push(Finding {
                instance,
                entity,
                attribute: Some(name),
                rule,
            });
        }
        Ok(Conformance {
            schema: Some(tables.name),
            findings: checker.findings,
            truncated: checker.truncated,
            where_rules: level3.counts,
            rules: rules::ALL
                .iter()
                .zip(counts)
                .map(|(&rule, [passed, failed, not_evaluated])| RuleCount {
                    rule,
                    passed,
                    failed,
                    not_evaluated,
                })
                .collect(),
        })
    }
}

const RULES: usize = rules::ALL.len();

struct Checker<'f> {
    file: &'f IfcFile,
    tables: &'static SchemaTables,
    positions: HashMap<u16, Vec<Position>>,
    states: [State; RULES],
    /// The (attribute, rule) pairs already reported for the current instance.
    reported: BTreeSet<(String, &'static str)>,
    findings: Vec<Finding>,
    truncated: bool,
    max_findings: usize,
    instance: u64,
    entity: String,
}

/// The number of instances per key of each `UNIQUE` rule, with the memory charge of the keys.
struct UniqueKeys {
    seen: BTreeMap<(u16, usize, String), u64>,
    used: u64,
    memory: Option<u64>,
    record_limit: u64,
}

impl Checker<'_> {
    fn start(&mut self, instance: &Instance) {
        self.states = [State::NotApplicable; RULES];
        self.reported.clear();
        self.instance = instance.name;
        self.entity = instance
            .records
            .iter()
            .map(|r| r.type_name.as_str())
            .collect::<Vec<_>>()
            .join("+");
    }

    fn set(&mut self, rule: Rule, state: State) {
        let s = &mut self.states[slot(rule)];
        *s = (*s).max(state);
    }

    fn pass(&mut self, rule: Rule) {
        self.set(rule, State::Passed);
    }

    fn fail(&mut self, rule: Rule, attribute: Option<&str>) {
        self.set(rule, State::Failed);
        let key = (attribute.unwrap_or_default().to_owned(), rule.code);
        if self.reported.contains(&key) {
            return;
        }
        if self.findings.len() >= self.max_findings {
            self.truncated = true;
            return;
        }
        self.reported.insert(key);
        self.findings.push(Finding {
            instance: self.instance,
            entity: self.entity.clone(),
            attribute: attribute.map(str::to_owned),
            rule,
        });
    }

    fn check(&mut self, rule: Rule, ok: bool, attribute: Option<&str>) {
        if ok {
            self.pass(rule);
        } else {
            self.fail(rule, attribute);
        }
    }

    fn positions(&mut self, entity: u16) -> Vec<Position> {
        let tables = self.tables;
        self.positions
            .entry(entity)
            .or_insert_with(|| tables.positions(entity))
            .clone()
    }

    /// Level 1 for one instance; returns the entity indexes of its parts when they are all known.
    fn structure(&mut self, instance: &Instance) -> Option<Vec<u16>> {
        let tables = self.tables;
        let mut parts = Vec::with_capacity(instance.records.len());
        for record in &instance.records {
            match tables.entity_index(&record.type_name) {
                Some(e) => parts.push(e),
                None => {
                    self.fail(rules::ENTITY_UNKNOWN, None);
                    self.set(rules::ENTITY_ABSTRACT, State::NotEvaluated);
                    self.set(rules::ATTRIBUTE_COUNT, State::NotEvaluated);
                    for rule in VALUE_RULES {
                        self.set(rule, State::NotEvaluated);
                    }
                    if instance.records.len() > 1 {
                        self.set(rules::COMPLEX_INVALID, State::NotEvaluated);
                    }
                    return None;
                }
            }
        }
        self.pass(rules::ENTITY_UNKNOWN);
        match (instance.records.as_slice(), parts.as_slice()) {
            ([record], [entity]) => {
                let abstract_ = tables
                    .entities
                    .get(usize::from(*entity))
                    .is_some_and(|e| e.is_abstract);
                self.check(rules::ENTITY_ABSTRACT, !abstract_, None);
                let positions = self.positions(*entity);
                self.record(record, &positions);
            }
            _ => {
                let valid = complex_valid(tables, &parts);
                self.check(rules::COMPLEX_INVALID, valid, None);
                for (record, &entity) in instance.records.iter().zip(&parts) {
                    let positions = own_positions(tables, entity, &parts);
                    self.record(record, &positions);
                }
            }
        }
        Some(parts)
    }

    /// The parameters of a record against its positions.
    fn record(&mut self, record: &Record, positions: &[Position]) {
        if positions.len() != record.parameters.len() {
            self.fail(rules::ATTRIBUTE_COUNT, None);
            for rule in VALUE_RULES {
                self.set(rule, State::NotEvaluated);
            }
            return;
        }
        self.pass(rules::ATTRIBUTE_COUNT);
        for (position, parameter) in positions.iter().zip(&record.parameters) {
            self.attribute(position, parameter);
        }
    }

    fn attribute(&mut self, position: &Position, parameter: &Parameter) {
        let name = position.attribute.name;
        if position.derived {
            self.check(
                rules::DERIVED_MISPLACED,
                matches!(parameter, Parameter::Derived),
                Some(name),
            );
            return;
        }
        match parameter {
            Parameter::Derived => self.fail(rules::DERIVED_MISPLACED, Some(name)),
            Parameter::Omitted => {
                self.pass(rules::DERIVED_MISPLACED);
                self.check(
                    rules::VALUE_MISSING,
                    position.attribute.optional,
                    Some(name),
                );
            }
            _ => {
                self.pass(rules::DERIVED_MISPLACED);
                self.pass(rules::VALUE_MISSING);
                self.value(position.attribute, position.attribute.ty, parameter, 0);
            }
        }
    }

    /// A present value against a type.
    fn value(&mut self, attribute: &Attribute, ty: TypeRef, parameter: &Parameter, depth: u32) {
        let name = Some(attribute.name);
        if depth > crate::step::MAX_NESTING {
            self.set(rules::VALUE_TYPE, State::NotEvaluated);
            return;
        }
        let tables = self.tables;
        let ty = tables.underlying(ty);
        if matches!(parameter, Parameter::Typed { .. })
            && !matches!(ty, TypeRef::Named(Named::Select(_)))
        {
            // A typed parameter only stands for a member of a select.
            return self.fail(rules::VALUE_TYPE, name);
        }
        match ty {
            TypeRef::Integer => self.kind(matches!(parameter, Parameter::Integer(_)), name),
            // An integer is a real number in EXPRESS.
            TypeRef::Real | TypeRef::Number => self.kind(
                matches!(parameter, Parameter::Integer(_) | Parameter::Real(_)),
                name,
            ),
            TypeRef::Boolean => self.kind(
                matches!(parameter, Parameter::Enumeration(e) if matches!(e.to_ascii_uppercase().as_str(), "T" | "F")),
                name,
            ),
            TypeRef::Logical => self.kind(
                matches!(parameter, Parameter::Enumeration(e) if matches!(e.to_ascii_uppercase().as_str(), "T" | "F" | "U")),
                name,
            ),
            TypeRef::String { width, fixed } => {
                if let Parameter::Text(text) = parameter {
                    self.pass(rules::VALUE_TYPE);
                    if let Some(width) = width {
                        let len = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
                        self.width(len, width, fixed, name);
                    }
                } else {
                    self.fail(rules::VALUE_TYPE, name);
                }
            }
            TypeRef::Binary { width, fixed } => {
                if let Parameter::Binary(digits) = parameter {
                    self.pass(rules::VALUE_TYPE);
                    if let Some(width) = width {
                        match binary_bits(digits) {
                            Some(bits) => self.width(bits, width, fixed, name),
                            None => self.fail(rules::VALUE_WIDTH, name),
                        }
                    }
                } else {
                    self.fail(rules::VALUE_TYPE, name);
                }
            }
            TypeRef::Named(Named::Entity(entity)) => {
                if let Parameter::Reference(target) = parameter {
                    self.pass(rules::VALUE_TYPE);
                    if let Some(parts) = self.target(*target, name) {
                        let ok = parts.iter().any(|&p| tables.is_subtype_of(p, entity));
                        self.check(rules::REFERENCE_TYPE, ok, name);
                    }
                } else {
                    self.fail(rules::VALUE_TYPE, name);
                }
            }
            TypeRef::Named(Named::Enumeration(i)) => {
                if let Parameter::Enumeration(item) = parameter {
                    self.pass(rules::VALUE_TYPE);
                    let ok = tables
                        .enumerations
                        .get(usize::from(i))
                        .is_some_and(|e| e.contains(item));
                    self.check(rules::ENUMERATION_ITEM, ok, name);
                } else {
                    self.fail(rules::VALUE_TYPE, name);
                }
            }
            TypeRef::Named(Named::Select(s)) => self.select(attribute, s, parameter, depth),
            // `underlying` stops on a chain it cannot follow.
            TypeRef::Named(Named::Defined(_)) => self.set(rules::VALUE_TYPE, State::NotEvaluated),
            TypeRef::Aggregate(aggregate) => {
                let Parameter::List(items) = parameter else {
                    return self.fail(rules::VALUE_TYPE, name);
                };
                self.pass(rules::VALUE_TYPE);
                let len = u64::try_from(items.len()).unwrap_or(u64::MAX);
                match size_ok(aggregate.kind, aggregate.lower, aggregate.upper, len) {
                    Some(ok) => self.check(rules::AGGREGATE_SIZE, ok, name),
                    None => self.set(rules::AGGREGATE_SIZE, State::NotEvaluated),
                }
                if aggregate.kind == AggregateKind::Set || aggregate.unique {
                    self.check(rules::AGGREGATE_UNIQUE, distinct(items), name);
                }
                let array_optional = aggregate.optional && aggregate.kind == AggregateKind::Array;
                for item in items {
                    match item {
                        Parameter::Omitted => {
                            self.check(rules::VALUE_MISSING, array_optional, name);
                        }
                        Parameter::Derived => self.fail(rules::DERIVED_MISPLACED, name),
                        _ => self.value(attribute, aggregate.element, item, depth + 1),
                    }
                }
            }
        }
    }

    fn kind(&mut self, ok: bool, name: Option<&str>) {
        self.check(rules::VALUE_TYPE, ok, name);
    }

    fn width(&mut self, len: u64, width: u32, fixed: bool, name: Option<&str>) {
        let width = u64::from(width);
        let ok = if fixed { len == width } else { len <= width };
        self.check(rules::VALUE_WIDTH, ok, name);
    }

    /// The entity parts of a referenced instance: `None`, with the rule marked, when it is
    /// missing or has an unknown part.
    fn target(&mut self, target: u64, name: Option<&str>) -> Option<Vec<u16>> {
        let Some(instance) = self.file.instance(target) else {
            self.fail(rules::REFERENCE_DANGLING, name);
            return None;
        };
        self.pass(rules::REFERENCE_DANGLING);
        let parts: Option<Vec<u16>> = instance
            .records
            .iter()
            .map(|r| self.tables.entity_index(&r.type_name))
            .collect();
        if parts.is_none() {
            self.set(rules::REFERENCE_TYPE, State::NotEvaluated);
            self.set(rules::SELECT_MEMBER, State::NotEvaluated);
        }
        parts
    }

    fn select(&mut self, attribute: &Attribute, select: u16, parameter: &Parameter, depth: u32) {
        let name = Some(attribute.name);
        let tables = self.tables;
        match parameter {
            Parameter::Reference(target) => {
                self.pass(rules::VALUE_TYPE);
                if let Some(parts) = self.target(*target, name) {
                    let ok = parts
                        .iter()
                        .any(|&p| tables.select_accepts_entity(select, p));
                    self.check(rules::SELECT_MEMBER, ok, name);
                }
            }
            Parameter::Typed { type_name, value } => {
                self.pass(rules::VALUE_TYPE);
                match tables.find(type_name) {
                    Some(member)
                        if !matches!(member, Named::Entity(_))
                            && tables.select_contains(select, member) =>
                    {
                        self.pass(rules::SELECT_MEMBER);
                        self.value(attribute, TypeRef::Named(member), value, depth + 1);
                    }
                    _ => self.fail(rules::SELECT_MEMBER, name),
                }
            }
            // A value of a select that is not an entity names its type.
            _ => self.fail(rules::SELECT_MEMBER, name),
        }
    }

    /// Level 2: the inverse attributes of the instance's entity and its supertypes, nearest
    /// declaration first.
    fn inverses(&mut self, instance: &Instance, parts: &[u16], index: &InverseIndex) {
        let tables = self.tables;
        let mut seen: Vec<&str> = Vec::new();
        let mut declared: Vec<&'static Inverse> = Vec::new();
        for &part in parts {
            for e in tables
                .supertypes(part)
                .filter_map(|e| tables.entities.get(usize::from(e)))
            {
                for inverse in e.inverses {
                    if !seen.iter().any(|n| n.eq_ignore_ascii_case(inverse.name)) {
                        seen.push(inverse.name);
                        declared.push(inverse);
                    }
                }
            }
        }
        for inverse in declared {
            let count = index
                .members(self.file, tables, instance.name, inverse)
                .len();
            let count = u64::try_from(count).unwrap_or(u64::MAX);
            let ok = match inverse.aggregate {
                None => Some(count == 1),
                Some(a) => size_ok(AggregateKind::Bag, a.lower, a.upper, count),
            };
            match ok {
                Some(ok) => self.check(rules::INVERSE_CARDINALITY, ok, Some(inverse.name)),
                None => self.set(rules::INVERSE_CARDINALITY, State::NotEvaluated),
            }
        }
    }

    /// Level 2: the `UNIQUE` rules of the instance's entity and its supertypes. Every instance of a
    /// group sharing the values fails; an instance without a value for one of the attributes is not
    /// compared.
    fn unique(&mut self, instance: &Instance, parts: &[u16], keys: &UniqueKeys) {
        let mut outcomes = Vec::new();
        unique_keys(
            self.file,
            self.tables,
            instance,
            parts,
            |owner, r, names, key| {
                let repeated = keys.seen.get(&(owner, r, key)).is_some_and(|&n| n > 1);
                outcomes.push((names, !repeated));
            },
        );
        for (names, ok) in outcomes {
            self.check(rules::UNIQUE_RULE, ok, Some(&names));
        }
    }
}

impl UniqueKeys {
    /// Counts the keys of every `UNIQUE` rule over the file, charging each new key.
    fn count(&mut self, file: &IfcFile, tables: &SchemaTables, cancel: &Cancel) -> Result<()> {
        for instance in file.instances() {
            cancel.check()?;
            let parts: Option<Vec<u16>> = instance
                .records
                .iter()
                .map(|r| tables.entity_index(&r.type_name))
                .collect();
            let Some(parts) = parts else { continue };
            let mut found = Vec::new();
            unique_keys(file, tables, instance, &parts, |owner, r, _, key| {
                found.push((owner, r, key));
            });
            for key in found {
                if let Some(n) = self.seen.get_mut(&key) {
                    *n = n.saturating_add(1);
                    continue;
                }
                self.used = self
                    .used
                    .saturating_add(KEY_BYTES)
                    .saturating_add(u64::try_from(key.2.len()).unwrap_or(u64::MAX));
                if self.memory.is_some_and(|max| self.used > max) {
                    return Err(limit("memory_bytes", self.used));
                }
                if u64::try_from(self.seen.len()).unwrap_or(u64::MAX) >= self.record_limit {
                    return Err(limit("record_count", self.record_limit.saturating_add(1)));
                }
                self.seen.insert(key, 1);
            }
        }
        Ok(())
    }
}

/// Calls `found` with the declaring entity, the rule index, the attribute names and the key of
/// every `UNIQUE` rule that applies to the instance and for which it has every value.
fn unique_keys(
    file: &IfcFile,
    tables: &SchemaTables,
    instance: &Instance,
    parts: &[u16],
    mut found: impl FnMut(u16, usize, String, String),
) {
    let mut owners: Vec<u16> = Vec::new();
    for &part in parts {
        for owner in tables.supertypes(part) {
            // Parts of a complex instance share supertypes; each rule is taken once.
            if owners.contains(&owner) {
                continue;
            }
            owners.push(owner);
            let Some(e) = tables.entities.get(usize::from(owner)) else {
                continue;
            };
            for (r, rule) in e.unique.iter().enumerate() {
                let mut key = String::new();
                let complete = rule.attributes.iter().all(|written| {
                    let attribute = written.rsplit('.').next().unwrap_or(written);
                    match file.instance_attribute(instance, attribute) {
                        None | Some(Parameter::Omitted | Parameter::Derived) => false,
                        Some(value) => {
                            key.push_str(&format!("{value:?}\u{1f}"));
                            true
                        }
                    }
                });
                if complete {
                    found(owner, r, rule.attributes.join(","), key);
                }
            }
        }
    }
}

/// Whether `len` elements fit the bounds; `None` when a bound is an expression.
fn size_ok(kind: AggregateKind, lower: Bound, upper: Bound, len: u64) -> Option<bool> {
    let value = |b: Bound| match b {
        Bound::Value(v) => Some(Some(u64::from(v))),
        Bound::Unbounded => Some(None),
        Bound::Expression(_) => None,
    };
    let (lower, upper) = (value(lower)?, value(upper)?);
    Some(match kind {
        // An array holds one element per index of its range.
        AggregateKind::Array => match (lower, upper) {
            (Some(l), Some(u)) => len == u.saturating_sub(l).saturating_add(1),
            _ => true,
        },
        _ => lower.is_none_or(|l| len >= l) && upper.is_none_or(|u| len <= u),
    })
}

/// Whether no element of a list occurs twice.
fn distinct(items: &[Parameter]) -> bool {
    let names: Option<Vec<u64>> = items
        .iter()
        .map(|i| match i {
            Parameter::Reference(n) => Some(*n),
            _ => None,
        })
        .collect();
    if let Some(mut names) = names {
        names.sort_unstable();
        return names.windows(2).all(|w| w[0] != w[1]);
    }
    let mut seen = BTreeSet::new();
    items.iter().all(|i| seen.insert(format!("{i:?}")))
}

/// The number of bits of a binary literal: four per digit after the first, less the unused bits
/// the first digit gives. `None` when the first digit is not 0 to 3.
fn binary_bits(digits: &str) -> Option<u64> {
    let mut chars = digits.chars();
    let unused = u64::from(chars.next()?.to_digit(16)?);
    if unused > 3 {
        return None;
    }
    let rest = u64::try_from(chars.count()).ok()?;
    (rest * 4).checked_sub(unused)
}

/// Whether the parts of a complex instance form a valid combination: every part's supertype is a
/// part, no two parts are alternatives of one `ONEOF`, and every part that has no subtype among the
/// parts is concrete.
fn complex_valid(tables: &SchemaTables, parts: &[u16]) -> bool {
    let entity = |i: u16| tables.entities.get(usize::from(i));
    let closed = parts.iter().all(|&p| {
        entity(p)
            .and_then(|e| e.supertype)
            .is_none_or(|s| parts.contains(&s))
    });
    let one_of = parts.iter().all(|&p| {
        entity(p).is_some_and(|e| e.one_of.iter().filter(|s| parts.contains(s)).count() <= 1)
    });
    let leaves_concrete = parts.iter().all(|&p| {
        let has_subtype = parts
            .iter()
            .any(|&q| entity(q).and_then(|e| e.supertype) == Some(p));
        has_subtype || entity(p).is_some_and(|e| !e.is_abstract)
    });
    closed && one_of && leaves_concrete
}

/// The positions of a part of a complex instance: the attributes its entity declares, each
/// derived where any part re-declares it.
fn own_positions(tables: &SchemaTables, entity: u16, parts: &[u16]) -> Vec<Position> {
    let Some(e) = tables.entities.get(usize::from(entity)) else {
        return Vec::new();
    };
    e.attributes
        .iter()
        .enumerate()
        .filter_map(|(i, attribute)| {
            let declared = super::AttributeRef {
                entity,
                index: u16::try_from(i).ok()?,
            };
            let derived = parts
                .iter()
                .filter_map(|&p| tables.entities.get(usize::from(p)))
                .flat_map(|p| p.derived)
                .any(|d| d.redeclares == Some(declared));
            Some(Position {
                declared,
                attribute,
                derived,
            })
        })
        .collect()
}
