//! Embedded outline fonts of the PDF writer (CORE-IO-037): one subset TrueType font for each font
//! that the outline glyph runs of a document draw with.
//!
//! The subset keeps the font's glyph identifiers. Glyphs that are drawn, and the components of
//! composite glyphs, keep their outlines; every other glyph has an empty outline. The content
//! stream therefore addresses the embedded font with the font's own identifiers (`Identity-H`
//! and `CIDToGIDMap /Identity`). Every byte is read through bounds-checked accessors, so a
//! malformed font is a [`Refusal`], never a panic.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::sync::Arc;

use arc_base::Limits;
use arc_draft::fonts::OutlineFont;
use arc_draft::plot::{ItemKind, PlotStream};
use skrifa::{FontRef, MetadataProvider, Tag};

/// The objects of one embedded font: the composite font, its descendant font, the font
/// descriptor, the font file and the ToUnicode map.
pub(crate) const OBJECTS_PER_FONT: usize = 5;

/// The OS/2 `fsType` bits that forbid embedding: restricted license (0x0002), no subsetting
/// (0x0100) and bitmap embedding only (0x0200).
const FS_REFUSED: u16 = 0x0302;

/// The most component references one subset may follow in total; a font with more is refused.
const COMPONENT_BUDGET: usize = 1_000_000;

/// The most character map entries one ToUnicode map may read; a font with more is refused.
const MAPPING_BUDGET: usize = 2_000_000;

/// The most bytes of a BaseFont name (PDF limits names to 127 bytes).
const NAME_BYTES: usize = 127;

/// The bit of a composite component's flags for two word-sized arguments.
const ARGS_ARE_WORDS: u16 = 0x0001;
/// The bit of a composite component's flags for a single scale.
const HAS_SCALE: u16 = 0x0008;
/// The bit of a composite component's flags for separate x and y scales.
const HAS_XY_SCALE: u16 = 0x0040;
/// The bit of a composite component's flags for a two by two transform.
const HAS_TWO_BY_TWO: u16 = 0x0080;
/// The bit of a composite component's flags for a following component.
const MORE_COMPONENTS: u16 = 0x0020;

/// Why the glyphs of a font are not written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The embedding permissions of the font forbid embedding it.
    Restricted,
    /// The outlines are not TrueType glyph data (for example CFF), which this writer does not embed.
    Unsupported,
    /// The bytes are not a font this writer can read, or exceed the limits, or name a glyph the font lacks.
    Invalid,
}

/// The objects of one embedded font, as the writer emits them.
#[derive(Debug, PartialEq)]
pub(crate) struct Program {
    /// The base font name: a six-letter subset tag, `+`, then the family name in ASCII letters and digits.
    pub name: String,
    /// The subset font file (TrueType tables).
    pub file: Vec<u8>,
    /// The entries of the `W` array of the descendant font.
    pub widths: String,
    /// The entries of the font descriptor, without the font file reference.
    pub descriptor: String,
    /// The ToUnicode CMap.
    pub to_unicode: Vec<u8>,
}

/// One font of the document, with the glyphs drawn from it and the program that embeds it.
#[derive(Debug)]
pub(crate) struct Used {
    /// The program, or why the glyphs are not written.
    pub program: Result<Program, Refusal>,
}

/// The fonts of a document in order of family name and face index.
#[derive(Debug, Default)]
pub(crate) struct Fonts {
    list: Vec<Used>,
    /// The list position of each font by the address and length of its bytes, which identify it
    /// within one document.
    by_address: BTreeMap<(usize, usize), usize>,
}

/// The identity of a font within one document: family, face index, byte length and digest. Two
/// files under one family name are two fonts.
type FontKey = (String, u32, usize, u64);

/// The address and length of the bytes of `font`.
fn address_of(font: &OutlineFont) -> (usize, usize) {
    (
        Arc::as_ptr(&font.data).cast::<u8>() as usize,
        font.data.len(),
    )
}

impl Fonts {
    /// Collects the outline fonts that the items of `streams` draw with and embeds each of them.
    /// The digest of each distinct byte slice is computed once.
    pub(crate) fn collect(streams: &[&PlotStream], limits: &Limits) -> Self {
        let mut digests: BTreeMap<(usize, usize), u64> = BTreeMap::new();
        let mut found: BTreeMap<FontKey, (OutlineFont, BTreeSet<u32>)> = BTreeMap::new();
        let mut keys: BTreeMap<(usize, usize), FontKey> = BTreeMap::new();
        for stream in streams {
            for item in &stream.items {
                if let ItemKind::Glyphs { font, glyphs, .. } = &item.kind {
                    let address = address_of(font);
                    let digest = *digests.entry(address).or_insert_with(|| fnv(&font.data));
                    let key = (font.family.clone(), font.index, font.data.len(), digest);
                    keys.entry(address).or_insert_with(|| key.clone());
                    let entry = found
                        .entry(key)
                        .or_insert_with(|| (font.clone(), BTreeSet::new()));
                    entry.1.extend(glyphs.iter().map(|g| g.id));
                }
            }
        }
        let mut out = Self::default();
        let mut position = BTreeMap::new();
        for (key, (font, glyphs)) in found {
            let program = program(&font, &glyphs, limits);
            position.insert(key, out.list.len());
            out.list.push(Used { program });
        }
        for (address, key) in keys {
            if let Some(at) = position.get(&key) {
                out.by_address.insert(address, *at);
            }
        }
        out
    }

    /// The position of `font` in the list, and its entry; `None` for a font `collect` did not see.
    pub(crate) fn get(&self, font: &OutlineFont) -> Option<(usize, &Used)> {
        let at = *self.by_address.get(&address_of(font))?;
        self.list.get(at).map(|used| (at, used))
    }

    /// The fonts with a program, in list order.
    pub(crate) fn embedded(&self) -> impl Iterator<Item = (usize, &Program)> {
        self.list
            .iter()
            .enumerate()
            .filter_map(|(at, used)| used.program.as_ref().ok().map(|p| (at, p)))
    }
}

/// The program of `font` drawing the glyphs `glyphs`.
fn program(
    font: &OutlineFont,
    glyphs: &BTreeSet<u32>,
    limits: &Limits,
) -> Result<Program, Refusal> {
    let size = u64::try_from(font.data.len()).unwrap_or(u64::MAX);
    if size > limits.input_bytes {
        return Err(Refusal::Invalid);
    }
    let face = FontRef::from_index(&font.data, font.index).map_err(|_| Refusal::Invalid)?;
    let table = |tag: &[u8; 4]| face.table_data(Tag::new(tag)).map(|d| d.as_bytes());
    if let Some(os2) = table(b"OS/2") {
        // fsType is the u16 at offset 8; an OS/2 table that cannot hold it is malformed.
        let fs_type = u16_at(os2, 8).ok_or(Refusal::Invalid)?;
        if fs_type & FS_REFUSED != 0 {
            return Err(Refusal::Restricted);
        }
    }
    let (Some(glyf), Some(loca)) = (table(b"glyf"), table(b"loca")) else {
        let cff = table(b"CFF ").is_some() || table(b"CFF2").is_some();
        return Err(if cff {
            Refusal::Unsupported
        } else {
            Refusal::Invalid
        });
    };
    let (Some(head), Some(maxp), Some(hhea), Some(hmtx)) = (
        table(b"head"),
        table(b"maxp"),
        table(b"hhea"),
        table(b"hmtx"),
    ) else {
        return Err(Refusal::Invalid);
    };
    let font_tables = Tables::read(head, maxp, hhea, hmtx, loca, glyf, limits)?;
    let keep = font_tables.keep(glyphs)?;
    let file = font_tables.subset(&keep, [table(b"cvt "), table(b"fpgm"), table(b"prep")])?;
    let tag = subset_tag(&file);
    // Family names are ASCII letters and digits after the filter, so characters are bytes.
    let family: String = font
        .family
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(NAME_BYTES - tag.len() - 1)
        .collect();
    let base = if family.is_empty() {
        "Font".to_owned()
    } else {
        family
    };
    let name = format!("{tag}+{base}");
    let to_unicode = to_unicode(&face, glyphs)?;
    let scale = |v: i64| thousandths(v, font_tables.upem);
    let bbox: Vec<String> = [36, 38, 40, 42]
        .iter()
        .map(|at| scale(i64::from(i16_at(head, *at).unwrap_or(0))).to_string())
        .collect();
    let ascent = scale(i64::from(i16_at(hhea, 4).unwrap_or(0)));
    let descent = scale(i64::from(i16_at(hhea, 6).unwrap_or(0)));
    let cap_height = table(b"OS/2")
        .and_then(|os2| i16_at(os2, 88))
        .map_or(ascent, |v| scale(i64::from(v)));
    let italic = table(b"post")
        .and_then(|post| i32_at(post, 4))
        .map_or(0.0, |v| f64::from(v) / 65536.0);
    let widths = glyphs.iter().try_fold(String::new(), |mut out, g| {
        let (advance, _) = font_tables.metric(usize::try_from(*g).ok()?)?;
        let _ = write!(out, " {g} [{}]", scale(i64::from(advance)));
        Some(out)
    });
    let descriptor = format!(
        "/Flags 32 /FontBBox [{}] /ItalicAngle {italic} /Ascent {ascent} /Descent {descent} /CapHeight {cap_height} /StemV 80",
        bbox.join(" ")
    );
    Ok(Program {
        name,
        file,
        widths: widths.ok_or(Refusal::Invalid)?.trim_start().to_owned(),
        descriptor,
        to_unicode,
    })
}

/// The tables of a TrueType font that the subset reads, with the counts they imply.
struct Tables<'a> {
    head: &'a [u8],
    maxp: &'a [u8],
    hhea: &'a [u8],
    hmtx: &'a [u8],
    loca: &'a [u8],
    glyf: &'a [u8],
    /// The number of glyphs.
    glyphs: usize,
    /// The number of horizontal metrics; the last advance repeats for the glyphs after them.
    metrics: usize,
    /// Units per em.
    upem: u16,
    /// Whether the location table holds long offsets.
    long: bool,
}

impl<'a> Tables<'a> {
    fn read(
        head: &'a [u8],
        maxp: &'a [u8],
        hhea: &'a [u8],
        hmtx: &'a [u8],
        loca: &'a [u8],
        glyf: &'a [u8],
        limits: &Limits,
    ) -> Result<Self, Refusal> {
        let upem = u16_at(head, 18)
            .filter(|u| *u > 0)
            .ok_or(Refusal::Invalid)?;
        let long = i16_at(head, 50).ok_or(Refusal::Invalid)? == 1;
        let glyphs = usize::from(u16_at(maxp, 4).ok_or(Refusal::Invalid)?);
        let limit = usize::try_from(limits.array_length).unwrap_or(usize::MAX);
        let metrics = usize::from(u16_at(hhea, 34).ok_or(Refusal::Invalid)?);
        let width = if long { 4 } else { 2 };
        if glyphs == 0 || glyphs > limit || metrics == 0 || metrics > glyphs {
            return Err(Refusal::Invalid);
        }
        if loca.len() < (glyphs + 1) * width || hmtx.len() < metrics * 4 + (glyphs - metrics) * 2 {
            return Err(Refusal::Invalid);
        }
        Ok(Self {
            head,
            maxp,
            hhea,
            hmtx,
            loca,
            glyf,
            glyphs,
            metrics,
            upem,
            long,
        })
    }

    /// The byte range of glyph `g` in the glyph data.
    fn range(&self, g: usize) -> Option<(usize, usize)> {
        let (from, to) = if self.long {
            (
                usize::try_from(u32_at(self.loca, g.checked_mul(4)?)?).ok()?,
                usize::try_from(u32_at(self.loca, g.checked_mul(4)?.checked_add(4)?)?).ok()?,
            )
        } else {
            (
                2 * usize::from(u16_at(self.loca, g.checked_mul(2)?)?),
                2 * usize::from(u16_at(self.loca, g.checked_mul(2)?.checked_add(2)?)?),
            )
        };
        (from <= to && to <= self.glyf.len()).then_some((from, to))
    }

    /// The advance width and left side bearing of glyph `g`.
    fn metric(&self, g: usize) -> Option<(u16, i16)> {
        if g < self.metrics {
            Some((u16_at(self.hmtx, g * 4)?, i16_at(self.hmtx, g * 4 + 2)?))
        } else {
            let last = u16_at(self.hmtx, (self.metrics - 1) * 4)?;
            let side = i16_at(self.hmtx, self.metrics * 4 + (g - self.metrics) * 2)?;
            Some((last, side))
        }
    }

    /// The glyphs to keep: those drawn, the composite components they use, and glyph 0.
    fn keep(&self, drawn: &BTreeSet<u32>) -> Result<Vec<bool>, Refusal> {
        let mut keep = vec![false; self.glyphs];
        let mut budget = COMPONENT_BUDGET;
        let mut stack = vec![0_usize];
        for id in drawn {
            stack.push(usize::try_from(*id).map_err(|_| Refusal::Invalid)?);
        }
        while let Some(g) = stack.pop() {
            let slot = keep.get_mut(g).ok_or(Refusal::Invalid)?;
            if *slot {
                continue;
            }
            *slot = true;
            let (from, to) = self.range(g).ok_or(Refusal::Invalid)?;
            let data = self.glyf.get(from..to).ok_or(Refusal::Invalid)?;
            for component in components(data).ok_or(Refusal::Invalid)? {
                budget = budget.checked_sub(1).ok_or(Refusal::Invalid)?;
                stack.push(usize::from(component));
            }
        }
        Ok(keep)
    }

    /// The subset font file: the tables of the subset, with the glyphs not kept empty.
    fn subset(&self, keep: &[bool], extra: [Option<&[u8]>; 3]) -> Result<Vec<u8>, Refusal> {
        let mut glyf = Vec::new();
        let mut loca = Vec::with_capacity((self.glyphs + 1) * 4);
        let mut hmtx = Vec::with_capacity(self.glyphs * 4);
        for (g, kept) in keep.iter().enumerate() {
            loca.extend_from_slice(
                &u32::try_from(glyf.len())
                    .map_err(|_| Refusal::Invalid)?
                    .to_be_bytes(),
            );
            let (advance, side) = if *kept {
                let (from, to) = self.range(g).ok_or(Refusal::Invalid)?;
                let data = self.glyf.get(from..to).ok_or(Refusal::Invalid)?;
                // Ranges may overlap, so the copies are bounded by the source before each append.
                if glyf.len() + data.len() > self.glyf.len() {
                    return Err(Refusal::Invalid);
                }
                glyf.extend_from_slice(data);
                while glyf.len() % 4 != 0 {
                    glyf.push(0);
                }
                self.metric(g).ok_or(Refusal::Invalid)?
            } else {
                (0, 0)
            };
            hmtx.extend_from_slice(&advance.to_be_bytes());
            hmtx.extend_from_slice(&side.to_be_bytes());
        }
        loca.extend_from_slice(
            &u32::try_from(glyf.len())
                .map_err(|_| Refusal::Invalid)?
                .to_be_bytes(),
        );
        let mut head = self.head.get(..54).ok_or(Refusal::Invalid)?.to_vec();
        put(&mut head, 8, &[0; 4]);
        put(&mut head, 50, &1_i16.to_be_bytes());
        let mut hhea = self.hhea.get(..36).ok_or(Refusal::Invalid)?.to_vec();
        put(
            &mut hhea,
            34,
            &u16::try_from(self.glyphs)
                .map_err(|_| Refusal::Invalid)?
                .to_be_bytes(),
        );
        let mut tables: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"glyf", glyf),
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"hmtx", hmtx),
            (*b"loca", loca),
            (*b"maxp", self.maxp.to_vec()),
        ];
        for (tag, data) in [b"cvt ", b"fpgm", b"prep"].iter().zip(extra) {
            if let Some(data) = data {
                tables.push((**tag, data.to_vec()));
            }
        }
        assemble(tables).ok_or(Refusal::Invalid)
    }
}

/// The glyph identifiers of the components of a composite glyph; empty for a simple glyph.
fn components(glyph: &[u8]) -> Option<Vec<u16>> {
    if glyph.is_empty() || i16_at(glyph, 0)? >= 0 {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let mut at = 10;
    loop {
        let flags = u16_at(glyph, at)?;
        out.push(u16_at(glyph, at.checked_add(2)?)?);
        at += 4 + if flags & ARGS_ARE_WORDS != 0 { 4 } else { 2 };
        at += if flags & HAS_SCALE != 0 {
            2
        } else if flags & HAS_XY_SCALE != 0 {
            4
        } else if flags & HAS_TWO_BY_TWO != 0 {
            8
        } else {
            0
        };
        if flags & MORE_COMPONENTS == 0 {
            return Some(out);
        }
    }
}

/// Assembles the sfnt file of `tables`, sorted by tag, with the checksums of the specification.
fn assemble(mut tables: Vec<([u8; 4], Vec<u8>)>) -> Option<Vec<u8>> {
    tables.sort_by_key(|(tag, _)| *tag);
    let count = u16::try_from(tables.len()).ok()?;
    let mut power = 1_u16;
    let mut log = 0_u16;
    while power * 2 <= count {
        power *= 2;
        log += 1;
    }
    let search = power * 16;
    let mut file = Vec::new();
    file.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
    for field in [count, search, log, count * 16 - search] {
        file.extend_from_slice(&field.to_be_bytes());
    }
    let mut offset = 12 + 16 * tables.len();
    let mut head_at = None;
    for (tag, data) in &tables {
        if tag == b"head" {
            head_at = Some(offset);
        }
        file.extend_from_slice(tag);
        file.extend_from_slice(&checksum(data).to_be_bytes());
        file.extend_from_slice(&u32::try_from(offset).ok()?.to_be_bytes());
        file.extend_from_slice(&u32::try_from(data.len()).ok()?.to_be_bytes());
        offset += data.len().div_ceil(4) * 4;
    }
    for (_, data) in &tables {
        file.extend_from_slice(data);
        while file.len() % 4 != 0 {
            file.push(0);
        }
    }
    let at = head_at? + 8;
    let adjust = 0xB1B0_AFBA_u32.wrapping_sub(checksum(&file));
    put(&mut file, at, &adjust.to_be_bytes());
    Some(file)
}

/// The sum of the big-endian words of `bytes`, padded with zeros to a word.
fn checksum(bytes: &[u8]) -> u32 {
    bytes.chunks(4).fold(0_u32, |sum, chunk| {
        let mut word = [0_u8; 4];
        put(&mut word, 0, chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// A six-letter tag derived from the bytes of the subset.
fn subset_tag(file: &[u8]) -> String {
    let hash = fnv(file);
    (0..6)
        .map(|i| char::from(b'A' + u8::try_from((hash >> (i * 5)) % 26).unwrap_or(0)))
        .collect()
}

/// The ToUnicode CMap of the glyphs: each drawn glyph maps to the lowest Unicode scalar value the
/// font's character map assigns to it, if any.
fn to_unicode(face: &FontRef<'_>, glyphs: &BTreeSet<u32>) -> Result<Vec<u8>, Refusal> {
    unicode_map(
        face.charmap()
            .mappings()
            .map(|(code, gid)| (code, gid.to_u32())),
        glyphs,
    )
}

/// The ToUnicode CMap of `mappings`, pairs of code point and glyph identifier, for the glyphs
/// `glyphs`. Code points that are not scalar values are skipped; more than `MAPPING_BUDGET`
/// mappings are refused.
fn unicode_map(
    mappings: impl Iterator<Item = (u32, u32)>,
    glyphs: &BTreeSet<u32>,
) -> Result<Vec<u8>, Refusal> {
    let mut first: BTreeMap<u32, u32> = BTreeMap::new();
    for (read, (code, gid)) in mappings.enumerate() {
        if read >= MAPPING_BUDGET {
            return Err(Refusal::Invalid);
        }
        if glyphs.contains(&gid) && char::from_u32(code).is_some() {
            first
                .entry(gid)
                .and_modify(|c| *c = (*c).min(code))
                .or_insert(code);
        }
    }
    let mut out = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let pairs: Vec<(u32, char)> = first
        .into_iter()
        .filter_map(|(gid, code)| Some((gid, char::from_u32(code)?)))
        .collect();
    for block in pairs.chunks(100) {
        let _ = writeln!(out, "{} beginbfchar", block.len());
        for (gid, c) in block {
            let mut units = [0_u16; 2];
            let hex: String = c
                .encode_utf16(&mut units)
                .iter()
                .map(|u| format!("{u:04X}"))
                .collect();
            let _ = writeln!(out, "<{gid:04X}> <{hex}>");
        }
        out.push_str("endbfchar\n");
    }
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    Ok(out.into_bytes())
}

/// `v` in thousandths of an em for units per em `upem`, rounded half away from zero.
fn thousandths(v: i64, upem: u16) -> i64 {
    let upem = i64::from(upem.max(1));
    let n = v * 1000;
    let half = upem / 2;
    if n >= 0 {
        (n + half) / upem
    } else {
        -((-n + half) / upem)
    }
}

/// A 64-bit FNV-1a digest of `bytes`.
fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, b| {
        (hash ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Writes `bytes` at `at` of `buf`, where the range fits; the callers check their offsets.
fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
    if let Some(target) = at
        .checked_add(bytes.len())
        .and_then(|end| buf.get_mut(at..end))
    {
        target.copy_from_slice(bytes);
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    be::<2>(b, at).map(u16::from_be_bytes)
}

fn i16_at(b: &[u8], at: usize) -> Option<i16> {
    be::<2>(b, at).map(i16::from_be_bytes)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    be::<4>(b, at).map(u32::from_be_bytes)
}

fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    be::<4>(b, at).map(i32::from_be_bytes)
}

fn be<const N: usize>(b: &[u8], at: usize) -> Option<[u8; N]> {
    b.get(at..at.checked_add(N)?)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn unit_values_round_half_away_from_zero() {
        assert_eq!(thousandths(2, 3), 667);
        assert_eq!(thousandths(-2, 3), -667);
        assert_eq!(thousandths(1, 2000), 1);
        assert_eq!(thousandths(1, 2001), 0);
    }

    #[test]
    fn surrogates_are_skipped_and_the_budget_is_enforced() {
        let glyphs = BTreeSet::from([1, 2]);
        let cmap = unicode_map([(0xD800, 1), (0x41, 1), (0x42, 2)].into_iter(), &glyphs).unwrap();
        let text = String::from_utf8_lossy(&cmap).into_owned();
        assert!(text.contains("<0001> <0041>"));
        assert!(text.contains("<0002> <0042>"));
        assert!(!text.contains("<>") && !text.contains("D800"));
        let many = (0..=MAPPING_BUDGET).map(|c| (u32::try_from(c % 0xD000).unwrap_or(0), 1));
        assert_eq!(unicode_map(many, &glyphs), Err(Refusal::Invalid));
    }

    /// A TrueType file with glyph data `glyf` and the glyph offsets `offsets` (one more than the
    /// number of glyphs), built from the tables the subset reads.
    fn font_of(glyf: &[u8], offsets: &[u32]) -> OutlineFont {
        let glyphs = offsets.len() - 1;
        let mut head = vec![0_u8; 54];
        put(&mut head, 18, &1000_u16.to_be_bytes());
        put(&mut head, 50, &1_i16.to_be_bytes());
        let mut maxp = vec![0, 0, 0x50, 0];
        maxp.extend_from_slice(&u16::try_from(glyphs).unwrap().to_be_bytes());
        let mut hhea = vec![0_u8; 36];
        put(&mut hhea, 34, &u16::try_from(glyphs).unwrap().to_be_bytes());
        let hmtx = vec![0_u8; glyphs * 4];
        let loca: Vec<u8> = offsets.iter().flat_map(|o| o.to_be_bytes()).collect();
        let data = assemble(vec![
            (*b"glyf", glyf.to_vec()),
            (*b"head", head),
            (*b"hhea", hhea),
            (*b"hmtx", hmtx),
            (*b"loca", loca),
            (*b"maxp", maxp),
        ])
        .unwrap();
        OutlineFont {
            family: "Synthetic".to_owned(),
            data: Arc::from(data),
            index: 0,
        }
    }

    #[test]
    fn overlapping_glyph_ranges_cannot_grow_the_subset_past_its_source() {
        // Even glyphs share one 10-byte outline: [0, 10). Odd glyphs have inverted ranges that
        // are never copied. Drawing the 1000 even glyphs would copy the outline 1000 times.
        let offsets: Vec<u32> = (0..=2000)
            .map(|k| if k % 2 == 0 { 0 } else { 10 })
            .collect();
        let font = font_of(&[0; 10], &offsets);
        let drawn: BTreeSet<u32> = (0..2000).step_by(2).collect();
        assert_eq!(
            program(&font, &drawn, &Limits::CONSTRAINED).map(|p| p.file.len()),
            Err(Refusal::Invalid)
        );
    }

    #[test]
    fn component_references_are_bounded_by_a_budget() {
        // Twenty composites, each with 65535 references to glyph 0: 1,310,700 references.
        let mut glyf = vec![0_u8; 10];
        let mut offsets = vec![0_u32, 10];
        for _ in 0..20 {
            glyf.extend_from_slice(&[0xFF, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0]);
            for c in 0..65_535_u32 {
                let more = if c + 1 < 65_535 { 0x0020_u16 } else { 0 };
                glyf.extend_from_slice(&more.to_be_bytes());
                glyf.extend_from_slice(&0_u16.to_be_bytes());
                glyf.extend_from_slice(&[0, 0]);
            }
            offsets.push(u32::try_from(glyf.len()).unwrap());
        }
        let font = font_of(&glyf, &offsets);
        let drawn: BTreeSet<u32> = (1..=20).collect();
        assert_eq!(
            program(&font, &drawn, &Limits::CONSTRAINED).map(|p| p.file.len()),
            Err(Refusal::Invalid)
        );
    }

    #[test]
    fn the_base_name_stays_within_127_bytes() {
        let font = OutlineFont {
            family: "A".repeat(300),
            data: arc_draft::fonts::bundled_outline().data,
            index: 0,
        };
        let program = program(&font, &BTreeSet::from([5]), &Limits::CONSTRAINED).unwrap();
        assert!(program.name.len() <= 127, "{}", program.name.len());
        assert_eq!(program.name.as_bytes()[6], b'+');
    }
}
