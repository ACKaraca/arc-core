//! Writing the neutral model as DXF (CORE-IO-003, CORE-IO-005, CORE-IO-006, CORE-IO-009,
//! CORE-IO-010).

use std::collections::BTreeSet;

use arc_base::{Cancel, Error, Result};

use crate::encoding::{RAW_BASE, Table as CodeTable, encode, table};
use crate::error::{codes, reasons};
use crate::fields::{Body, WriteCx};
use crate::group::{Group, Handle, Value, ValueKind, Version, value_kind};
use crate::loss::{Direction, LossHeader, LossLog, LossReport};
use crate::model::{
    Drawing, Entity, EntityKind, Head, Object, ObjectKind, PreservedRecord, SeqEnd, Table,
    TableEntry, XData,
};
use crate::read::Progress;
use crate::token::SENTINEL;

/// Options of [`write`].
pub struct WriteOptions<'a> {
    /// The target version, 2000 to 2018.
    pub version: Version,
    /// Whether to write the binary form instead of ASCII.
    pub binary: bool,
    /// Cancelling it makes the write return `base.cancelled` before the next record.
    pub cancel: Cancel,
    /// Called on the first record, every 1024 records and at the end.
    pub progress: Option<&'a mut dyn FnMut(Progress)>,
}

impl Default for WriteOptions<'_> {
    fn default() -> Self {
        Self {
            version: Version::R2018,
            binary: false,
            cancel: Cancel::new(),
            progress: None,
        }
    }
}

impl WriteOptions<'_> {
    /// ASCII options for a target version.
    #[must_use]
    pub fn for_version(version: Version) -> Self {
        Self {
            version,
            ..Self::default()
        }
    }
}

/// The result of a write.
#[derive(Debug, Clone, PartialEq)]
pub struct WriteOutput {
    /// The file bytes.
    pub bytes: Vec<u8>,
    /// The loss report.
    pub report: LossReport,
}

/// Writes a drawing. The bytes depend only on the drawing and the version and form chosen.
///
/// # Errors
///
/// `io.unsupported_version` for a target before 2000, `io.dxf.write_value_invalid` for a value
/// that cannot be written, and `base.cancelled`.
pub fn write(drawing: &Drawing, options: &mut WriteOptions<'_>) -> Result<WriteOutput> {
    let version = options.version;
    if version < Version::R2000 {
        return Err(Error::new(codes::UNSUPPORTED_VERSION).with_arg("version", version.name()));
    }
    let mut max = None::<u64>;
    let mut total = 0u64;
    visit(drawing, &mut |h| {
        total += 1;
        if let Some(h) = h {
            max = Some(max.map_or(h.0, |m| m.max(h.0)));
        }
    });
    let mut w = Writer {
        out: Vec::new(),
        version,
        binary: options.binary,
        legacy: (!version.is_unicode()).then(|| table(drawing.code_page())),
        emitted: BTreeSet::new(),
        next: max.map_or(1, |m| m.wrapping_add(1)),
        assigned: 0,
        raw_written: 0,
        loss: LossLog::bounded(),
        cancel: options.cancel.clone(),
        progress: options.progress.as_deref_mut(),
        done: 0,
        total,
    };
    if drawing.tables.block_records.is_none() {
        w.loss
            .add(reasons::STRUCTURE_INCOMPLETE, "BLOCK_RECORD", None, 1);
    }
    w.body(drawing)?;
    let body = std::mem::take(&mut w.out);
    if w.binary {
        w.out.extend_from_slice(SENTINEL);
    }
    w.header(drawing)?;
    w.out.extend_from_slice(&body);
    w.emit(&[Group::text(0, "EOF")])?;
    w.report_progress();
    let mut loss = w.loss;
    loss.add(reasons::HANDLE_ASSIGNED, "", None, w.assigned);
    loss.add(reasons::RAW_BYTES_WRITTEN, "", None, w.raw_written);
    let report = loss.finish(LossHeader {
        format: "dxf".to_owned(),
        version: version.name().to_owned(),
        direction: Direction::Export,
        file_name: None,
        file_hash: None,
        application: None,
        application_version: None,
        time: None,
        limits: None,
    });
    Ok(WriteOutput {
        bytes: w.out,
        report,
    })
}

/// Calls `f` once per record the writer emits, with the record's model handle.
fn visit(d: &Drawing, f: &mut dyn FnMut(Option<Handle>)) {
    fn entity(e: &Entity, f: &mut dyn FnMut(Option<Handle>)) {
        f(e.source_id());
        let (children, seqend) = match &e.kind {
            EntityKind::Polyline(p) => (&p.vertices, Some(&p.seqend)),
            EntityKind::Insert(i) if !i.attribs.is_empty() || i.seqend.is_some() => {
                (&i.attribs, Some(&i.seqend))
            }
            _ => return,
        };
        for c in children {
            entity(c, f);
        }
        if let Some(seqend) = seqend {
            match seqend {
                Some(s) => entity(s, f),
                None => f(None),
            }
        }
    }
    fn table<T>(t: &Option<Table<T>>, f: &mut dyn FnMut(Option<Handle>)) {
        if let Some(t) = t {
            f(t.head.handle);
            for e in &t.entries {
                f(e.head.handle);
            }
        }
    }
    for _ in &d.classes {
        f(None);
    }
    let t = &d.tables;
    table(&t.vports, f);
    table(&t.linetypes, f);
    table(&t.layers, f);
    table(&t.styles, f);
    table(&t.views, f);
    table(&t.ucs, f);
    table(&t.appids, f);
    table(&t.dimstyles, f);
    table(&t.block_records, f);
    for p in &t.preserved {
        f(None);
        for e in &p.entries {
            f(e.handle());
        }
    }
    for b in &d.blocks {
        entity(&b.begin, f);
        for e in &b.entities {
            entity(e, f);
        }
        entity(&b.end, f);
    }
    for e in &d.entities {
        entity(e, f);
    }
    for o in &d.objects {
        f(o.source_id());
    }
    for s in &d.preserved_sections {
        for _ in &s.records {
            f(None);
        }
    }
}

struct Writer<'o, 'p> {
    out: Vec<u8>,
    version: Version,
    binary: bool,
    /// `None` for UTF-8; the legacy table otherwise (`Some(None)` for an unimplemented page).
    legacy: Option<Option<&'static CodeTable>>,
    emitted: BTreeSet<u64>,
    next: u64,
    assigned: u64,
    raw_written: u64,
    loss: LossLog,
    cancel: Cancel,
    progress: Option<&'o mut (dyn FnMut(Progress) + 'p)>,
    done: u64,
    total: u64,
}

impl Writer<'_, '_> {
    fn report_progress(&mut self) {
        let (done, total) = (self.done, self.total);
        if let Some(progress) = self.progress.as_mut() {
            progress(Progress { done, total });
        }
    }

    fn tick(&mut self) -> Result<()> {
        self.cancel.check()?;
        self.done += 1;
        if self.done % 1024 == 1 {
            self.report_progress();
        }
        Ok(())
    }

    /// The handle a record is written with: its own unless missing or already written.
    fn assign(&mut self, handle: Option<Handle>) -> Handle {
        if let Some(h) = handle
            && self.emitted.insert(h.0)
        {
            return h;
        }
        while !self.emitted.insert(self.next) {
            self.next = self.next.wrapping_add(1);
        }
        let h = Handle(self.next);
        self.next = self.next.wrapping_add(1);
        self.assigned += 1;
        h
    }

    fn emit(&mut self, groups: &[Group]) -> Result<()> {
        for g in groups {
            self.group(g)?;
        }
        Ok(())
    }

    fn group(&mut self, g: &Group) -> Result<()> {
        let kind = value_kind(g.code);
        let invalid = || {
            Error::new(codes::DXF_WRITE_VALUE_INVALID).with_detail("group_code", i64::from(g.code))
        };
        let valid = match (kind, &g.value) {
            (Some(ValueKind::Text), Value::Text(_)) | (Some(ValueKind::Bool), Value::Bool(_)) => {
                true
            }
            (None, Value::Text(_)) => !self.binary && g.code >= 0,
            (Some(ValueKind::Double), Value::Double(v)) => v.is_finite(),
            (Some(ValueKind::Int8), Value::Int(v)) => (0..=255).contains(v),
            (Some(k @ (ValueKind::Int16 | ValueKind::Int32 | ValueKind::Int64)), Value::Int(v)) => {
                crate::token::int_in_range(k, *v) == Some(*v)
            }
            (Some(ValueKind::Bytes), Value::Bytes(b)) => !self.binary || b.len() <= 255,
            _ => false,
        };
        if !valid {
            return Err(invalid());
        }
        if self.binary {
            self.out.extend_from_slice(&g.code.to_le_bytes());
            match (&g.value, kind) {
                (Value::Text(t), _) => {
                    let bytes = self.encode(t);
                    self.out.extend_from_slice(&bytes);
                    self.out.push(0);
                }
                (Value::Double(v), _) => self.out.extend_from_slice(&v.to_le_bytes()),
                (Value::Bool(b), _) => self.out.push(u8::from(*b)),
                (Value::Bytes(b), _) => {
                    self.out.push(u8::try_from(b.len()).map_err(|_| invalid())?);
                    self.out.extend_from_slice(b);
                }
                (Value::Int(v), Some(ValueKind::Int8)) => {
                    self.out.push(u8::try_from(*v).map_err(|_| invalid())?);
                }
                (Value::Int(v), Some(ValueKind::Int16)) => self
                    .out
                    .extend_from_slice(&i16::try_from(*v).map_err(|_| invalid())?.to_le_bytes()),
                (Value::Int(v), Some(ValueKind::Int32)) => self
                    .out
                    .extend_from_slice(&i32::try_from(*v).map_err(|_| invalid())?.to_le_bytes()),
                (Value::Int(v), _) => self.out.extend_from_slice(&v.to_le_bytes()),
            }
            return Ok(());
        }
        self.out
            .extend_from_slice(format!("{:>3}\r\n", g.code).as_bytes());
        match &g.value {
            Value::Text(t) => {
                let bytes = self.encode(t);
                self.out.extend_from_slice(&bytes);
            }
            // `Debug` prints the shortest text that parses back to the same `f64`.
            Value::Double(v) => self.out.extend_from_slice(format!("{v:?}").as_bytes()),
            Value::Int(v) => self.out.extend_from_slice(v.to_string().as_bytes()),
            Value::Bool(b) => self.out.push(if *b { b'1' } else { b'0' }),
            Value::Bytes(b) => {
                for byte in b {
                    self.out.extend_from_slice(format!("{byte:02X}").as_bytes());
                }
            }
        }
        self.out.extend_from_slice(b"\r\n");
        Ok(())
    }

    fn encode(&mut self, text: &str) -> Vec<u8> {
        if self.legacy.is_none() {
            let raw = text
                .chars()
                .filter(|&c| (RAW_BASE + 0x80..RAW_BASE + 0x100).contains(&u32::from(c)))
                .count();
            self.raw_written += u64::try_from(raw).unwrap_or(u64::MAX);
        }
        encode(text, self.legacy)
    }

    fn header(&mut self, d: &Drawing) -> Result<()> {
        let mut g = vec![
            Group::text(0, "SECTION"),
            Group::text(2, "HEADER"),
            Group::text(9, "$ACADVER"),
            Group::text(1, self.version.code()),
        ];
        let seed = Handle(self.next).to_hex();
        for var in &d.header {
            g.push(Group::text(9, var.name.clone()));
            if var.name == "$HANDSEED" {
                g.extend(var.values.iter().map(|v| match v.value {
                    Value::Text(_) => Group::text(v.code, seed.clone()),
                    _ => v.clone(),
                }));
            } else {
                g.extend(var.values.iter().cloned());
            }
        }
        g.push(Group::text(0, "ENDSEC"));
        self.emit(&g)
    }

    fn body(&mut self, d: &Drawing) -> Result<()> {
        self.section("CLASSES")?;
        for class in &d.classes {
            self.tick()?;
            let mut g = vec![Group::text(0, "CLASS")];
            crate::fields::Fields::put_fields(class, &mut g);
            g.extend(class.extras.iter().cloned());
            self.emit(&g)?;
        }
        self.end_section()?;

        self.section("TABLES")?;
        let t = &d.tables;
        self.table("VPORT", 5, t.vports.as_ref())?;
        self.table("LTYPE", 5, t.linetypes.as_ref())?;
        self.table("LAYER", 5, t.layers.as_ref())?;
        self.table("STYLE", 5, t.styles.as_ref())?;
        self.table("VIEW", 5, t.views.as_ref())?;
        self.table("UCS", 5, t.ucs.as_ref())?;
        self.table("APPID", 5, t.appids.as_ref())?;
        self.table("DIMSTYLE", 105, t.dimstyles.as_ref())?;
        self.table("BLOCK_RECORD", 5, t.block_records.as_ref())?;
        for p in &t.preserved {
            self.tick()?;
            let mut g = vec![Group::text(0, "TABLE"), Group::text(2, p.name.clone())];
            g.extend(p.groups.iter().cloned());
            self.emit(&g)?;
            for e in &p.entries {
                self.preserved(e)?;
            }
            self.emit(&[Group::text(0, "ENDTAB")])?;
        }
        self.end_section()?;

        self.section("BLOCKS")?;
        for b in &d.blocks {
            self.entity(&b.begin, None)?;
            for e in &b.entities {
                self.entity(e, None)?;
            }
            self.entity(&b.end, None)?;
        }
        self.end_section()?;

        self.section("ENTITIES")?;
        for e in &d.entities {
            self.entity(e, None)?;
        }
        self.end_section()?;

        self.section("OBJECTS")?;
        for o in &d.objects {
            self.object(o)?;
        }
        self.end_section()?;

        for s in &d.preserved_sections {
            self.section(&s.name)?;
            self.emit(&s.leading)?;
            for r in &s.records {
                self.tick()?;
                let mut g = vec![Group::text(0, r.kind.clone())];
                g.extend(r.groups.iter().cloned());
                self.emit(&g)?;
            }
            self.end_section()?;
        }
        Ok(())
    }

    fn section(&mut self, name: &str) -> Result<()> {
        self.emit(&[Group::text(0, "SECTION"), Group::text(2, name)])
    }

    fn end_section(&mut self) -> Result<()> {
        self.emit(&[Group::text(0, "ENDSEC")])
    }

    /// The type, handle, application groups and owner of a record; returns the handle written.
    fn head(
        &mut self,
        g: &mut Vec<Group>,
        kind: &str,
        handle_code: i16,
        head: &Head,
        remap: Option<(Handle, Handle)>,
    ) -> Handle {
        g.push(Group::text(0, kind));
        let h = self.assign(head.handle);
        g.push(Group::text(handle_code, h.to_hex()));
        g.extend(head.app_groups.iter().cloned());
        if let Some(owner) = head.owner {
            let owner = match remap {
                Some((old, new)) if old == owner => new,
                _ => owner,
            };
            g.push(Group::text(330, owner.to_hex()));
        }
        h
    }

    fn tail(g: &mut Vec<Group>, extras: &[Group], xdata: &[XData]) {
        g.extend(extras.iter().cloned());
        for x in xdata {
            g.push(Group::text(1001, x.app.clone()));
            g.extend(x.groups.iter().cloned());
        }
    }

    fn preserved(&mut self, r: &PreservedRecord) -> Result<()> {
        self.tick()?;
        let mut g = vec![Group::text(0, r.kind.clone())];
        let mut groups = r.groups.clone();
        if let Some(old) = r.handle() {
            let new = self.assign(Some(old));
            if new != old
                && let Some(slot) = groups.iter_mut().find(|g| g.code == 5)
            {
                *slot = Group::text(5, new.to_hex());
            }
        }
        g.extend(groups);
        self.emit(&g)
    }

    fn table<T: Body>(
        &mut self,
        name: &str,
        handle_code: i16,
        table: Option<&Table<T>>,
    ) -> Result<()> {
        let Some(table) = table else {
            return Ok(());
        };
        self.tick()?;
        let mut g = vec![];
        self.head(&mut g, "TABLE", 5, &table.head, None);
        g.insert(1, Group::text(2, name));
        g.push(Group::marker("AcDbSymbolTable"));
        g.push(crate::fields::count(70, table.entries.len()));
        if name == "DIMSTYLE" {
            g.push(Group::marker("AcDbDimStyleTable"));
        }
        Self::tail(&mut g, &table.extras, &table.xdata);
        self.emit(&g)?;
        for entry in &table.entries {
            self.entry(name, handle_code, entry)?;
        }
        self.emit(&[Group::text(0, "ENDTAB")])
    }

    fn entry<T: Body>(&mut self, name: &str, handle_code: i16, e: &TableEntry<T>) -> Result<()> {
        self.tick()?;
        let mut g = vec![];
        self.head(&mut g, name, handle_code, &e.head, None);
        g.push(Group::marker("AcDbSymbolTableRecord"));
        let mut data = vec![];
        let mut w = WriteCx {
            version: self.version,
            loss: &mut self.loss,
        };
        e.data.put(&mut data, &mut w)?;
        let split = usize::from(data.first().is_some_and(|d| d.code == 100));
        let mut data = data.into_iter();
        g.extend(data.by_ref().take(split));
        g.push(Group::text(2, e.name.clone()));
        g.push(Group::int(70, i64::from(e.flags)));
        g.extend(data);
        Self::tail(&mut g, &e.extras, &e.xdata);
        self.emit(&g)
    }

    fn entity(&mut self, e: &Entity, remap: Option<(Handle, Handle)>) -> Result<()> {
        let Some(body) = e.kind.body_ref() else {
            if let EntityKind::Preserved(r) = &e.kind {
                self.preserved(r)?;
            }
            return Ok(());
        };
        self.tick()?;
        let mut g = vec![];
        let h = self.head(&mut g, e.kind.type_name(), 5, &e.head, remap);
        g.push(Group::marker("AcDbEntity"));
        e.props.put_main(&mut g);
        g.extend(e.kind.lead_markers().iter().map(|m| Group::marker(m)));
        let mut w = WriteCx {
            version: self.version,
            loss: &mut self.loss,
        };
        body.put(&mut g, &mut w)?;
        e.props.put_tail(&mut g);
        Self::tail(&mut g, &e.extras, &e.xdata);
        self.emit(&g)?;

        let child_remap = match e.head.handle {
            Some(old) if old != h => Some((old, h)),
            _ => None,
        };
        let (children, seqend) = match &e.kind {
            EntityKind::Polyline(p) => (&p.vertices, &p.seqend),
            EntityKind::Insert(i) if !i.attribs.is_empty() || i.seqend.is_some() => {
                (&i.attribs, &i.seqend)
            }
            _ => return Ok(()),
        };
        for c in children {
            self.entity(c, child_remap)?;
        }
        match seqend {
            Some(s) => self.entity(s, child_remap),
            None => self.entity(&Entity::new(EntityKind::SeqEnd(SeqEnd::default())), None),
        }
    }

    fn object(&mut self, o: &Object) -> Result<()> {
        let Some(body) = o.kind.body_ref() else {
            if let ObjectKind::Preserved(r) = &o.kind {
                self.preserved(r)?;
            }
            return Ok(());
        };
        self.tick()?;
        let mut g = vec![];
        self.head(&mut g, o.kind.type_name(), 5, &o.head, None);
        let mut w = WriteCx {
            version: self.version,
            loss: &mut self.loss,
        };
        body.put(&mut g, &mut w)?;
        Self::tail(&mut g, &o.extras, &o.xdata);
        self.emit(&g)
    }
}
