//! Reading DXF bytes into the neutral model (CORE-IO-002, CORE-IO-003, CORE-IO-006,
//! CORE-IO-008, CORE-IO-010).

use arc_base::{Cancel, Error, Limits, Result};

use crate::encoding::{Decoder, is_supported};
use crate::error::{Position, codes, limit, reasons};
use crate::fields::{Body, Cursor, Cx, Fields};
use crate::group::{Group, Value, Version};
use crate::loss::{Argument, Direction, LossHeader, LossLog, LossReport, Source};
use crate::model::{
    Block, Class, Drawing, Entity, EntityKind, EntityProps, Head, HeaderVariable, Object,
    ObjectKind, PreservedRecord, PreservedSection, PreservedTable, Table, TableEntry, Tables,
    XData,
};
use crate::token::Tokens;

/// How far a read or write has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Bytes read, or records written.
    pub done: u64,
    /// Total bytes, or total records.
    pub total: u64,
}

/// Options of [`read`].
pub struct ReadOptions<'a> {
    /// The limits profile; [`Limits::CONSTRAINED`] by default.
    pub limits: Limits,
    /// Cancelling it makes the read return `base.cancelled` before the next record.
    pub cancel: Cancel,
    /// Called on the first record, every 1024 records and at the end.
    pub progress: Option<&'a mut dyn FnMut(Progress)>,
}

impl Default for ReadOptions<'_> {
    fn default() -> Self {
        Self {
            limits: Limits::CONSTRAINED,
            cancel: Cancel::new(),
            progress: None,
        }
    }
}

impl ReadOptions<'_> {
    /// Options with the given limits profile.
    #[must_use]
    pub fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }
}

/// The result of a read.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadOutput {
    /// The drawing.
    pub drawing: Drawing,
    /// The loss report.
    pub report: LossReport,
}

/// Reads a DXF file, ASCII or binary, from bytes.
///
/// # Errors
///
/// `io.limit_exceeded` naming the limit, `io.unsupported_version`, the `io.dxf.*` codes for
/// malformed input with the line or byte offset as detail, and `base.cancelled`.
pub fn read(data: &[u8], options: &mut ReadOptions<'_>) -> Result<ReadOutput> {
    let limits = options.limits;
    let size = u64::try_from(data.len()).unwrap_or(u64::MAX);
    if size > limits.input_bytes {
        return Err(limit("input_bytes", size));
    }
    let (acadver, code_page) = prescan(data, &limits)?;
    let version = match acadver {
        None => Version::R12,
        Some(code) => Version::from_code(&code)
            .ok_or_else(|| Error::new(codes::UNSUPPORTED_VERSION).with_arg("version", code))?,
    };
    let code_page = code_page.unwrap_or_else(|| "ANSI_1252".to_owned());
    let decoder = if version.is_unicode() {
        Decoder::Utf8
    } else {
        Decoder::legacy(&code_page)
    };
    let mut reader = Reader {
        tok: Tokens::new(data, decoder, &limits),
        limits: &limits,
        pending: None,
        peeked: None,
        records: 0,
        last: Position::Line(0),
        version,
        loss: LossLog::unbounded(),
        cancel: options.cancel.clone(),
        progress: options.progress.as_deref_mut(),
        total: size,
    };
    let drawing = reader.drawing()?;
    reader.report_progress();
    let mut loss = reader.loss;
    let raw = reader.tok.raw_bytes;
    if raw > 0 {
        let reason = if is_supported(&code_page) {
            reasons::BYTE_UNMAPPED
        } else {
            reasons::CODE_PAGE_UNSUPPORTED
        };
        loss.add_with(
            reason,
            "",
            None,
            raw,
            vec![("code_page".to_owned(), Argument::Text(code_page))],
        );
    }
    loss.add(reasons::COMMENT_DROPPED, "", None, reader.tok.comments);
    let report = loss.finish(LossHeader {
        format: "dxf".to_owned(),
        version: version.name().to_owned(),
        direction: Direction::Import,
        file_name: None,
        file_hash: None,
        application: None,
        application_version: None,
        time: None,
        limits: Some(limits),
    });
    Ok(ReadOutput { drawing, report })
}

/// Reads a DXF file from a stream, taking at most the input-size limit plus one byte.
///
/// # Errors
///
/// `io.stream_failed` when the stream fails, and every error of [`read`].
pub fn read_stream(
    mut stream: impl std::io::Read,
    options: &mut ReadOptions<'_>,
) -> Result<ReadOutput> {
    use std::io::Read;
    let mut data = Vec::new();
    stream
        .by_ref()
        .take(options.limits.input_bytes.saturating_add(1))
        .read_to_end(&mut data)
        .map_err(|e| Error::new(codes::STREAM_FAILED).with_source(e))?;
    read(&data, options)
}

/// Finds `$ACADVER` and `$DWGCODEPAGE` in a leading HEADER section, before text is decoded.
fn prescan(data: &[u8], limits: &Limits) -> Result<(Option<String>, Option<String>)> {
    let mut tok = Tokens::new(data, Decoder::Legacy(None), limits);
    let mut version = None;
    let mut code_page = None;
    let first = [tok.next()?, tok.next()?];
    let is_header = matches!(
        &first,
        [Some((a, _)), Some((b, _))] if a.as_text() == Some("SECTION") && b.code == 2 && b.as_text() == Some("HEADER")
    );
    if !is_header {
        return Ok((None, None));
    }
    let mut name = String::new();
    while let Some((g, _)) = tok.next()? {
        match g.code {
            0 => break,
            9 => name = g.as_text().unwrap_or_default().to_owned(),
            _ => {
                let slot = match name.as_str() {
                    "$ACADVER" => &mut version,
                    "$DWGCODEPAGE" => &mut code_page,
                    _ => continue,
                };
                if slot.is_none() {
                    *slot = g.as_text().map(str::to_owned);
                }
            }
        }
    }
    Ok((version, code_page))
}

/// One record: the groups from a group 0 up to the next.
pub(crate) struct Record {
    pub(crate) kind: String,
    pub(crate) at: Position,
    pub(crate) groups: Vec<Group>,
}

struct Reader<'a, 'o, 'p> {
    tok: Tokens<'a>,
    limits: &'a Limits,
    pending: Option<(Group, Position)>,
    peeked: Option<Record>,
    records: u64,
    last: Position,
    version: Version,
    loss: LossLog,
    cancel: Cancel,
    progress: Option<&'o mut (dyn FnMut(Progress) + 'p)>,
    total: u64,
}

impl Reader<'_, '_, '_> {
    fn report_progress(&mut self) {
        let done = u64::try_from(self.tok.offset()).unwrap_or(u64::MAX);
        if let Some(progress) = self.progress.as_mut() {
            progress(Progress {
                done,
                total: self.total,
            });
        }
    }

    fn fetch(&mut self) -> Result<Option<Record>> {
        let first = match self.pending.take() {
            Some(pending) => Some(pending),
            None => self.tok.next()?,
        };
        let Some((g, at)) = first else {
            return Ok(None);
        };
        self.last = at;
        if g.code != 0 {
            return Err(at.error(codes::DXF_STRUCTURE_INVALID));
        }
        self.records += 1;
        if self.records > self.limits.record_count {
            return Err(limit("record_count", self.records));
        }
        self.cancel.check()?;
        if self.records % 1024 == 1 {
            self.report_progress();
        }
        let kind = g.as_text().unwrap_or_default().to_owned();
        let mut groups = Vec::new();
        while let Some((g, at)) = self.tok.next()? {
            if g.code == 0 {
                self.pending = Some((g, at));
                break;
            }
            self.last = at;
            let n = u64::try_from(groups.len()).unwrap_or(u64::MAX) + 1;
            if n > self.limits.array_length {
                return Err(limit("array_length", n));
            }
            groups.push(g);
        }
        Ok(Some(Record { kind, at, groups }))
    }

    fn next_record(&mut self) -> Result<Option<Record>> {
        match self.peeked.take() {
            Some(record) => Ok(Some(record)),
            None => self.fetch(),
        }
    }

    /// The next record, failing with `io.dxf.truncated` at the end of the input.
    fn required(&mut self) -> Result<Record> {
        self.next_record()?
            .ok_or_else(|| self.last.error(codes::DXF_TRUNCATED))
    }

    fn peek_is(&mut self, kind: &str) -> Result<bool> {
        if self.peeked.is_none() {
            self.peeked = self.fetch()?;
        }
        Ok(self.peeked.as_ref().is_some_and(|r| r.kind == kind))
    }

    fn depth(&self, depth: u32) -> Result<()> {
        if depth > self.limits.nesting_depth {
            return Err(limit("nesting_depth", u64::from(depth)));
        }
        Ok(())
    }

    fn cx(&self, at: Position) -> Cx<'_> {
        Cx {
            version: self.version,
            limits: self.limits,
            at,
        }
    }

    fn drawing(&mut self) -> Result<Drawing> {
        let mut d = Drawing {
            version: self.version,
            ..Drawing::default()
        };
        while let Some(rec) = self.next_record()? {
            match rec.kind.as_str() {
                "EOF" => break,
                "SECTION" => self.section(rec, &mut d)?,
                _ => return Err(rec.at.error(codes::DXF_STRUCTURE_INVALID)),
            }
        }
        Ok(d)
    }

    fn section(&mut self, rec: Record, d: &mut Drawing) -> Result<()> {
        self.depth(1)?;
        let mut groups = rec.groups.into_iter();
        let name = match groups.next() {
            Some(g) if g.code == 2 => g.as_text().unwrap_or_default().to_owned(),
            _ => return Err(rec.at.error(codes::DXF_STRUCTURE_INVALID)),
        };
        let rest: Vec<Group> = groups.collect();
        match name.as_str() {
            "HEADER" => {
                header(rest, rec.at, &mut d.header)?;
                let end = self.required()?;
                if end.kind != "ENDSEC" {
                    return Err(end.at.error(codes::DXF_STRUCTURE_INVALID));
                }
            }
            "CLASSES" => loop {
                let r = self.required()?;
                match r.kind.as_str() {
                    "ENDSEC" => break,
                    "CLASS" => {
                        self.depth(2)?;
                        let class = self.class(&r)?;
                        d.classes.push(class);
                    }
                    _ => return Err(r.at.error(codes::DXF_STRUCTURE_INVALID)),
                }
            },
            "TABLES" => loop {
                let r = self.required()?;
                match r.kind.as_str() {
                    "ENDSEC" => break,
                    "TABLE" => self.table(r, &mut d.tables)?,
                    _ => return Err(r.at.error(codes::DXF_STRUCTURE_INVALID)),
                }
            },
            "BLOCKS" => loop {
                let r = self.required()?;
                match r.kind.as_str() {
                    "ENDSEC" => break,
                    "BLOCK" => {
                        let begin = self.entity(r, 2)?;
                        let (entities, end) = self.entities("ENDBLK", 3)?;
                        let end = self.entity(end, 2)?;
                        d.blocks.push(Block {
                            begin,
                            entities,
                            end,
                        });
                    }
                    _ => return Err(r.at.error(codes::DXF_STRUCTURE_INVALID)),
                }
            },
            "ENTITIES" => {
                let (entities, _) = self.entities("ENDSEC", 2)?;
                d.entities.extend(entities);
            }
            "OBJECTS" => loop {
                let r = self.required()?;
                if r.kind == "ENDSEC" {
                    break;
                }
                self.depth(2)?;
                let object = self.object(&r)?;
                d.objects.push(object);
            },
            _ => {
                let mut section = PreservedSection {
                    name,
                    leading: rest,
                    records: Vec::new(),
                };
                loop {
                    let r = self.required()?;
                    if r.kind == "ENDSEC" {
                        break;
                    }
                    self.depth(2)?;
                    section.records.push(PreservedRecord {
                        kind: r.kind,
                        groups: r.groups,
                    });
                }
                self.loss
                    .add(reasons::SECTION_PRESERVED, &section.name, None, 1);
                d.preserved_sections.push(section);
            }
        }
        Ok(())
    }

    /// Entities up to the record of kind `end`, which is returned.
    fn entities(&mut self, end: &str, depth: u32) -> Result<(Vec<Entity>, Record)> {
        let mut out = Vec::new();
        loop {
            let r = self.required()?;
            if r.kind == end {
                return Ok((out, r));
            }
            if matches!(r.kind.as_str(), "ENDSEC" | "ENDBLK" | "EOF" | "SECTION") {
                return Err(r.at.error(codes::DXF_STRUCTURE_INVALID));
            }
            out.push(self.entity(r, depth)?);
        }
    }

    /// One entity with its vertex or attribute records.
    fn entity(&mut self, rec: Record, depth: u32) -> Result<Entity> {
        self.depth(depth)?;
        let attribs_follow = rec
            .groups
            .iter()
            .any(|g| g.code == 66 && matches!(g.value, Value::Int(1) | Value::Bool(true)));
        let mut e = self.parse_entity(&rec, depth)?;
        let (children, seqend, child) = match &mut e.kind {
            EntityKind::Polyline(p) => (&mut p.vertices, &mut p.seqend, "VERTEX"),
            EntityKind::Insert(i) if attribs_follow => (&mut i.attribs, &mut i.seqend, "ATTRIB"),
            _ => return Ok(e),
        };
        while self.peek_is(child)? {
            let r = self.required()?;
            self.depth(depth + 1)?;
            children.push(self.parse_entity(&r, depth + 1)?);
        }
        if self.peek_is("SEQEND")? {
            let r = self.required()?;
            self.depth(depth + 1)?;
            *seqend = Some(Box::new(self.parse_entity(&r, depth + 1)?));
        }
        Ok(e)
    }

    fn parse_entity(&mut self, rec: &Record, depth: u32) -> Result<Entity> {
        let Some(mut kind) = EntityKind::from_type(&rec.kind) else {
            let record = preserve(rec);
            self.loss.add(
                reasons::RECORD_PRESERVED,
                &rec.kind,
                record.handle().map(|h| Source::Hex(h.0)),
                1,
            );
            return Ok(Entity {
                head: Head::default(),
                props: EntityProps::default(),
                kind: EntityKind::Preserved(record),
                extras: Vec::new(),
                xdata: Vec::new(),
            });
        };
        let mut head = Head::default();
        let mut props = EntityProps {
            layer: "0".to_owned(),
            ..EntityProps::default()
        };
        let mut extras = Vec::new();
        let mut xdata = Vec::new();
        let cx = self.cx(rec.at);
        if let Some(body) = kind.body() {
            parse_record(
                rec,
                &cx,
                depth,
                5,
                &mut head,
                &mut extras,
                &mut xdata,
                &mut |g, cur, sub| {
                    if body.take_more(g, cur, &cx, sub)? || props.take_field(g) {
                        return Ok(true);
                    }
                    Ok(body.take_field(g))
                },
            )?;
        }
        self.note(&rec.kind, head.handle, &extras);
        Ok(Entity {
            head,
            props,
            kind,
            extras,
            xdata,
        })
    }

    fn note(&mut self, kind: &str, handle: Option<crate::group::Handle>, extras: &[Group]) {
        self.loss.add_mapped(1);
        let n = u64::try_from(extras.len()).unwrap_or(u64::MAX);
        self.loss.add(
            reasons::GROUPS_PRESERVED,
            kind,
            handle.map(|h| Source::Hex(h.0)),
            n,
        );
    }

    fn object(&mut self, rec: &Record) -> Result<Object> {
        let Some(mut kind) = ObjectKind::from_type(&rec.kind) else {
            let record = preserve(rec);
            self.loss.add(
                reasons::RECORD_PRESERVED,
                &rec.kind,
                record.handle().map(|h| Source::Hex(h.0)),
                1,
            );
            return Ok(Object::new(ObjectKind::Preserved(record)));
        };
        let mut object = Object::new(ObjectKind::Preserved(PreservedRecord::default()));
        let cx = self.cx(rec.at);
        if let Some(body) = kind.body() {
            parse_record(
                rec,
                &cx,
                2,
                5,
                &mut object.head,
                &mut object.extras,
                &mut object.xdata,
                &mut |g, cur, sub| Ok(body.take_more(g, cur, &cx, sub)? || body.take_field(g)),
            )?;
        }
        object.kind = kind;
        self.note(&rec.kind, object.head.handle, &object.extras);
        Ok(object)
    }

    fn class(&mut self, rec: &Record) -> Result<Class> {
        let mut class = Class::default();
        let mut extras = Vec::new();
        let cx = self.cx(rec.at);
        parse_record(
            rec,
            &cx,
            2,
            -1,
            &mut Head::default(),
            &mut extras,
            &mut Vec::new(),
            &mut |g, _, _| Ok(class.take_field(g)),
        )?;
        self.note("CLASS", None, &extras);
        class.extras = extras;
        Ok(class)
    }

    fn table(&mut self, rec: Record, tables: &mut Tables) -> Result<()> {
        self.depth(2)?;
        let name = rec
            .groups
            .first()
            .filter(|g| g.code == 2)
            .and_then(Group::as_text)
            .ok_or_else(|| rec.at.error(codes::DXF_STRUCTURE_INVALID))?
            .to_owned();
        macro_rules! typed {
            ($slot:ident, $handle:literal) => {{
                let table = self.typed_table(&rec, &name, $handle)?;
                match &mut tables.$slot {
                    Some(existing) => existing.entries.extend(table.entries),
                    slot @ None => *slot = Some(table),
                }
            }};
        }
        match name.as_str() {
            "VPORT" => typed!(vports, 5),
            "LTYPE" => typed!(linetypes, 5),
            "LAYER" => typed!(layers, 5),
            "STYLE" => typed!(styles, 5),
            "VIEW" => typed!(views, 5),
            "UCS" => typed!(ucs, 5),
            "APPID" => typed!(appids, 5),
            "DIMSTYLE" => typed!(dimstyles, 105),
            "BLOCK_RECORD" => typed!(block_records, 5),
            _ => {
                let mut table = PreservedTable {
                    name,
                    groups: rec.groups.get(1..).unwrap_or_default().to_vec(),
                    entries: Vec::new(),
                };
                loop {
                    let r = self.required()?;
                    if r.kind == "ENDTAB" {
                        break;
                    }
                    self.depth(3)?;
                    table.entries.push(preserve(&r));
                }
                self.loss
                    .add(reasons::TABLE_PRESERVED, &table.name, None, 1);
                tables.preserved.push(table);
            }
        }
        Ok(())
    }

    fn typed_table<T: Body + Default>(
        &mut self,
        rec: &Record,
        name: &str,
        handle_code: i16,
    ) -> Result<Table<T>> {
        let mut table = Table::<T>::default();
        let cx = self.cx(rec.at);
        let mut named = false;
        let mut counted = false;
        parse_record(
            rec,
            &cx,
            2,
            5,
            &mut table.head,
            &mut table.extras,
            &mut table.xdata,
            &mut |g, _, _| {
                let drop = match g.code {
                    2 => !std::mem::replace(&mut named, true),
                    70 => !std::mem::replace(&mut counted, true),
                    _ => false,
                };
                Ok(drop)
            },
        )?;
        self.note(name, table.head.handle, &table.extras);
        loop {
            let r = self.required()?;
            if r.kind == "ENDTAB" {
                return Ok(table);
            }
            if r.kind != name {
                return Err(r.at.error(codes::DXF_STRUCTURE_INVALID));
            }
            self.depth(3)?;
            let mut entry = TableEntry::<T>::default();
            let cx = self.cx(r.at);
            let mut named = false;
            let mut flagged = false;
            let TableEntry {
                head,
                name: entry_name,
                flags,
                data,
                extras,
                xdata,
            } = &mut entry;
            parse_record(
                &r,
                &cx,
                3,
                handle_code,
                head,
                extras,
                xdata,
                &mut |g, cur, sub| {
                    if g.code == 2 && !named {
                        named = entry_name.take_field_text(g);
                        return Ok(named);
                    }
                    if g.code == 70
                        && !flagged
                        && let Value::Int(v) = g.value
                        && let Ok(v) = i16::try_from(v)
                    {
                        *flags = v;
                        flagged = true;
                        return Ok(true);
                    }
                    Ok(data.take_more(g, cur, &cx, sub)? || data.take_field(g))
                },
            )?;
            self.note(name, entry.head.handle, &entry.extras);
            table.entries.push(entry);
        }
    }
}

trait TakeText {
    fn take_field_text(&mut self, g: &Group) -> bool;
}

impl TakeText for String {
    fn take_field_text(&mut self, g: &Group) -> bool {
        match g.as_text() {
            Some(text) => {
                text.clone_into(self);
                true
            }
            None => false,
        }
    }
}

fn preserve(rec: &Record) -> PreservedRecord {
    PreservedRecord {
        kind: rec.kind.clone(),
        groups: rec.groups.clone(),
    }
}

fn header(groups: Vec<Group>, at: Position, out: &mut Vec<HeaderVariable>) -> Result<()> {
    for g in groups {
        if g.code == 9 {
            out.push(HeaderVariable {
                name: g.as_text().unwrap_or_default().to_owned(),
                values: Vec::new(),
            });
        } else {
            out.last_mut()
                .ok_or_else(|| at.error(codes::DXF_STRUCTURE_INVALID))?
                .values
                .push(g);
        }
    }
    out.retain(|v| v.name != "$ACADVER");
    Ok(())
}

/// Splits one record into head, typed groups (through `claim`), preserved groups and extended
/// data. `handle_code` is 5, 105 for dimension styles, or -1 for records without a head.
#[allow(clippy::too_many_arguments)]
fn parse_record(
    rec: &Record,
    cx: &Cx<'_>,
    depth: u32,
    handle_code: i16,
    head: &mut Head,
    extras: &mut Vec<Group>,
    xdata: &mut Vec<XData>,
    claim: &mut dyn FnMut(&Group, &mut Cursor<'_>, &str) -> Result<bool>,
) -> Result<()> {
    let mut cur = Cursor::new(&rec.groups);
    let mut sub = String::new();
    let mut marked = false;
    let mut braces = 0u32;
    let mut braces_in_head = false;
    let mut embedded = false;
    let nest = |braces: u32| {
        let total = depth.saturating_add(braces);
        if total > cx.limits.nesting_depth {
            Err(limit("nesting_depth", u64::from(total)))
        } else {
            Ok(())
        }
    };
    while let Some(g) = cur.next() {
        if g.code == 1001 {
            read_xdata(std::iter::once(g).chain(cur.rest()), &nest, xdata)?;
            break;
        }
        if g.code == 102 {
            let opening = g.as_text().is_some_and(|t| t.starts_with('{'));
            if braces == 0 {
                braces_in_head = !marked;
            }
            if opening {
                braces += 1;
                nest(braces)?;
            } else {
                braces = braces.saturating_sub(1);
            }
            if braces_in_head {
                &mut head.app_groups
            } else {
                &mut *extras
            }
            .push(g.clone());
            continue;
        }
        if braces > 0 {
            if braces_in_head {
                &mut head.app_groups
            } else {
                &mut *extras
            }
            .push(g.clone());
            continue;
        }
        if embedded {
            extras.push(g.clone());
            continue;
        }
        match g.code {
            100 => {
                g.as_text().unwrap_or_default().clone_into(&mut sub);
                marked = true;
                continue;
            }
            101 => {
                embedded = true;
                extras.push(g.clone());
                continue;
            }
            _ => {}
        }
        if !marked
            && g.code == handle_code
            && head.handle.is_none()
            && let Some(h) = g.as_handle()
        {
            head.handle = Some(h);
            continue;
        }
        if !marked
            && g.code == 330
            && head.owner.is_none()
            && let Some(h) = g.as_handle()
        {
            head.owner = Some(h);
            continue;
        }
        if !claim(g, &mut cur, &sub)? {
            extras.push(g.clone());
        }
    }
    Ok(())
}

/// Extended data from its first group 1001 to the end of the record.
fn read_xdata<'g>(
    groups: impl Iterator<Item = &'g Group>,
    nest: &dyn Fn(u32) -> Result<()>,
    xdata: &mut Vec<XData>,
) -> Result<()> {
    let mut braces = 0u32;
    for g in groups {
        match (g.code, g.as_text()) {
            (1001, Some(app)) => xdata.push(XData {
                app: app.to_owned(),
                groups: Vec::new(),
            }),
            (1002, Some(t)) => {
                if t.starts_with('{') {
                    braces += 1;
                    nest(braces)?;
                } else {
                    braces = braces.saturating_sub(1);
                }
                push_xdata(xdata, g);
            }
            _ => push_xdata(xdata, g),
        }
    }
    Ok(())
}

fn push_xdata(xdata: &mut [XData], g: &Group) {
    if let Some(last) = xdata.last_mut() {
        last.groups.push(g.clone());
    }
}
