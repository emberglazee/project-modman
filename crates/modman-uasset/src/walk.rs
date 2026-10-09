//! Byte-exact walker for cooked Project Wingman DataTables (UE4.27 tagged serialization).
//!
//! Mirrors UAssetAPI's read semantics (see the `ue4-reverse-engineering` skill
//! reference `cooked-datatable-format.md`). Produces a navigable tree with byte
//! ranges for every property so values can be surgically spliced in place
//! (see [`crate::edit`]) without re-serializing the file.
//!
//! Verified against `DB_Aircraft` (39 rows, 2,809 props) and
//! `DB_ProjectWingmanLevelList` (43 rows, 1,678 props): byte-exact walk with zero
//! leftover and zero diffs against UAssetAPI JSON for every value.

use crate::export::{read_export_map, ExportEntry};
use crate::header::PackageHeader;
use crate::names::read_name_table;
use crate::Error;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// A parsed cooked DataTable asset (.uasset + .uexp pair).
#[derive(Debug)]
pub struct DataTable {
    pub export: ExportEntry,
    pub names: Vec<String>,
    pub rows: Vec<Row>,
    /// The export object's own tagged properties (contains `RowStruct`).
    pub top_props: Vec<Prop>,
    /// Unconsumed payload bytes at the end of the export (must be 0 for a clean walk).
    pub leftover: usize,
    /// Size-field inconsistencies found while walking (must be empty for a clean walk).
    pub size_mismatches: Vec<String>,
    /// Per-kind counters (`props_total`, `prop:FloatProperty`, `customstruct:Vector`, ...).
    pub counters: HashMap<String, usize>,
}

/// One DataTable row: row name + tagged property list.
#[derive(Debug)]
pub struct Row {
    pub name: String,
    /// Offset of the row's name (FName) in the payload.
    pub start: usize,
    /// End (exclusive) of the row's property list (includes the `None` terminator).
    pub end: usize,
    pub props: Vec<Prop>,
}

/// A single tagged property with its byte range in the .uexp payload.
///
/// `vstart..vend` spans the bytes that a same-size splice may overwrite
/// (the raw value for fixed-size types; informational for struct/array/text).
#[derive(Debug, Clone)]
pub struct Prop {
    pub name: String,
    pub type_name: String,
    pub size: i32,
    pub array_index: i32,
    /// Offset of the property tag start (payload-relative).
    pub start: usize,
    /// Start of the value bytes (payload-relative).
    pub vstart: usize,
    /// End (exclusive) of the value bytes.
    pub vend: usize,
    /// Struct type name for `StructProperty` (e.g. `SAircraftStat`, `Vector`).
    pub struct_type: Option<String>,
    /// Set when this struct is a fixed-layout built-in (Vector, Guid, ...).
    pub custom: Option<String>,
    pub value: PropValue,
}

#[derive(Debug, Clone)]
pub enum PropValue {
    Int(i32),
    Float(f32),
    Bool(bool),
    Str(String),
    Name(NameRef),
    Object(i32),
    Byte {
        enum_name: String,
        value: ByteValue,
    },
    Text(TextValue),
    Struct {
        children: Vec<Prop>,
    },
    Custom(CustomValue),
    Array {
        elem_type: String,
        count: i32,
        items: Vec<ArrayElem>,
    },
}

/// A resolved FName (index + number + display string).
#[derive(Debug, Clone)]
pub struct NameRef {
    pub index: i32,
    pub number: i32,
    pub value: String,
}

#[derive(Debug, Clone)]
pub enum ByteValue {
    Byte(u8),
    FName(NameRef),
}

#[derive(Debug, Clone)]
pub struct TextValue {
    pub flags: u32,
    pub history: i8,
    pub namespace: Option<String>,
    pub key: Option<String>,
    pub source: Option<String>,
    pub table: Option<NameRef>,
}

#[derive(Debug, Clone)]
pub enum ArrayElem {
    /// Primitive item (with its byte span in the payload).
    Prim {
        value: PropValue,
        start: usize,
        end: usize,
    },
    /// Struct item (children carry their own spans).
    Struct(Vec<Prop>),
    /// Fixed-layout built-in struct item (with its byte span).
    Custom {
        kind: String,
        value: CustomValue,
        start: usize,
        end: usize,
    },
}

/// Decoded value of a fixed-layout built-in struct.
#[derive(Debug, Clone)]
pub enum CustomValue {
    Floats(Vec<f32>),
    Ints(Vec<i64>),
    Bytes(Vec<u8>),
}

/// Fixed-layout built-in struct kinds (name -> (kind, count)).
/// kind: `f` floats, `i` int32s, `b` raw bytes, `l` single int64, `d` single int32.
fn custom_kind(name: &str) -> Option<(char, usize)> {
    Some(match name {
        "Vector" | "Rotator" => ('f', 3),
        "Quat" | "LinearColor" | "Vector4" => ('f', 4),
        "Vector2D" => ('f', 2),
        "IntPoint" | "IntVector2" => ('i', 2),
        "IntVector" => ('i', 3),
        "Guid" => ('b', 16),
        "DateTime" | "Timespan" => ('l', 1),
        "Color" => ('d', 1),
        _ => return None,
    })
}

fn with_ext(p: &Path, ext: &str) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

impl DataTable {
    /// Load a `.uasset` + `.uexp` pair from a stem path (no extension).
    pub fn load(stem: impl AsRef<Path>) -> Result<Self, Error> {
        let stem = stem.as_ref();
        let ua = std::fs::read(with_ext(stem, "uasset"))?;
        let ue = std::fs::read(with_ext(stem, "uexp"))?;
        Self::walk_bytes(&ua, &ue)
    }

    /// Walk an in-memory `.uasset` + `.uexp` pair.
    pub fn walk_bytes(uasset: &[u8], uexp: &[u8]) -> Result<Self, Error> {
        let mut cursor = Cursor::new(uasset);
        let header = PackageHeader::read(&mut cursor)?;

        let names = {
            let mut c = Cursor::new(uasset);
            c.set_position(header.name_offset as u64);
            read_name_table(&mut c, header.name_count)?
        };
        let exports = {
            let mut c = Cursor::new(uasset);
            c.set_position(header.export_offset as u64);
            read_export_map(&mut c, header.export_count)?
        };
        if exports.len() != 1 {
            return Err(Error::Parse(format!(
                "expected exactly 1 export, found {}",
                exports.len()
            )));
        }
        let export = exports.into_iter().next().unwrap();

        let bias = export.serial_offset - uasset.len() as i64;
        if bias != 0 {
            return Err(Error::Parse(format!(
                "unexpected uexp bias {bias} (serial_offset={}, uasset_len={})",
                export.serial_offset,
                uasset.len()
            )));
        }
        let size = export.serial_size as usize;
        if size > uexp.len() {
            return Err(Error::Parse(format!(
                "export serial_size {size} exceeds uexp length {}",
                uexp.len()
            )));
        }

        let payload = &uexp[..size];
        let (top_props, rows, leftover, size_mismatches, counters) = {
            let mut w = Walker::new(payload, &names);
            let top_props = w.prop_list("export")?;
            let object_guid_present = w.i32()?;
            if object_guid_present == 1 {
                w.skip(16)?;
            }
            let num_rows = w.i32()?;
            let mut rows = Vec::new();
            for i in 0..num_rows {
                let row_start = w.pos;
                let rn = w.fname()?;
                let props = w.prop_list(&format!("row[{i}]{}", rn.value))?;
                let row_end = w.pos;
                rows.push(Row {
                    name: rn.value,
                    start: row_start,
                    end: row_end,
                    props,
                });
            }
            let leftover = payload.len().saturating_sub(w.pos);
            (top_props, rows, leftover, w.mismatches, w.counters)
        };

        Ok(DataTable {
            export,
            names,
            rows,
            top_props,
            leftover,
            size_mismatches,
            counters,
        })
    }

    /// Total number of properties in the tree (export props + rows + nested structs + struct-array items).
    pub fn count_props(&self) -> usize {
        count_in(&self.top_props) + self.rows.iter().map(|r| count_in(&r.props)).sum::<usize>()
    }
}

fn count_in(props: &[Prop]) -> usize {
    props
        .iter()
        .map(|p| {
            1 + match &p.value {
                PropValue::Struct { children } => count_in(children),
                PropValue::Array { items, .. } => items
                    .iter()
                    .map(|it| match it {
                        ArrayElem::Struct(c) => count_in(c),
                        _ => 0,
                    })
                    .sum(),
                _ => 0,
            }
        })
        .sum()
}

/// Find a row by exact name.
pub fn find_row<'a>(dt: &'a DataTable, name: &str) -> Option<&'a Row> {
    dt.rows.iter().find(|r| r.name == name)
}

/// Match a child property by segment: exact name, `Name_...` prefix, or `Name...` prefix.
pub fn find_child<'a>(props: &'a [Prop], seg: &str) -> Option<&'a Prop> {
    props
        .iter()
        .find(|p| p.name == seg || p.name.split('_').next() == Some(seg) || p.name.starts_with(seg))
}

/// Resolve `row` + dotted path (e.g. `["BaseStats", "MaxSpeed"]`) to a property.
pub fn resolve<'a>(dt: &'a DataTable, row: &str, path: &[&str]) -> Option<&'a Prop> {
    let mut props: &[Prop] = &find_row(dt, row)?.props;
    let mut node: Option<&Prop> = None;
    for (i, seg) in path.iter().enumerate() {
        let p = find_child(props, seg)?;
        node = Some(p);
        if i + 1 < path.len() {
            match &p.value {
                PropValue::Struct { children } => props = children,
                _ => return None,
            }
        }
    }
    node
}

struct Walker<'a> {
    b: &'a [u8],
    names: &'a [String],
    pos: usize,
    counters: HashMap<String, usize>,
    mismatches: Vec<String>,
}

impl<'a> Walker<'a> {
    fn new(b: &'a [u8], names: &'a [String]) -> Self {
        Self {
            b,
            names,
            pos: 0,
            counters: HashMap::new(),
            mismatches: Vec::new(),
        }
    }

    fn err(&self, msg: impl Into<String>) -> Error {
        Error::Parse(format!("{} @ 0x{:x}", msg.into(), self.pos))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.pos.checked_add(n).is_none_or(|end| end > self.b.len()) {
            return Err(self.err(format!("unexpected EOF reading {n} bytes")));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn i8(&mut self) -> Result<i8, Error> {
        Ok(self.take(1)?[0] as i8)
    }

    fn i32(&mut self) -> Result<i32, Error> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn i64(&mut self) -> Result<i64, Error> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn f32(&mut self) -> Result<f32, Error> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn skip(&mut self, n: usize) -> Result<(), Error> {
        self.take(n)?;
        Ok(())
    }

    fn fname(&mut self) -> Result<NameRef, Error> {
        let index = self.i32()?;
        let number = self.i32()?;
        let value = if index >= 0 && (index as usize) < self.names.len() {
            let base = &self.names[index as usize];
            if number > 0 {
                format!("{base}_{}", number - 1)
            } else {
                base.clone()
            }
        } else {
            format!("<bad:{index}:{number}>")
        };
        Ok(NameRef {
            index,
            number,
            value,
        })
    }

    fn fstring(&mut self) -> Result<String, Error> {
        let n = self.i32()?;
        self.fstring_body(n)
    }

    /// Read an FString body for an already-read length prefix.
    fn fstring_body(&mut self, n: i32) -> Result<String, Error> {
        if n == 0 {
            return Ok(String::new());
        }
        if n < 0 {
            let cnt = (-n) as usize;
            let bytes = self.take(cnt * 2)?;
            let mut u16s = Vec::with_capacity(cnt);
            let (chunks, _) = bytes.as_chunks::<2>();
            for c in chunks {
                u16s.push(u16::from_le_bytes(*c));
            }
            if let Some(&0) = u16s.last() {
                u16s.pop();
            }
            Ok(String::from_utf16_lossy(&u16s))
        } else {
            let bytes = self.take(n as usize)?;
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
        }
    }

    /// The 1-byte "has property GUID" flag present after every UE4.27 tag.
    fn skip_guid_flag(&mut self) -> Result<(), Error> {
        let has = self.u8()?;
        match has {
            0 => Ok(()),
            1 => {
                self.skip(16)?;
                Ok(())
            }
            other => Err(self.err(format!("invalid has-property-guid flag {other}"))),
        }
    }

    fn tick(&mut self, key: &str) {
        *self.counters.entry(key.to_string()).or_insert(0) += 1;
    }

    fn read_custom(&mut self, kind: char, count: usize) -> Result<CustomValue, Error> {
        Ok(match kind {
            'f' => {
                let mut v = Vec::with_capacity(count);
                for _ in 0..count {
                    v.push(self.f32()?);
                }
                CustomValue::Floats(v)
            }
            'i' => {
                let mut v = Vec::with_capacity(count);
                for _ in 0..count {
                    v.push(self.i32()? as i64);
                }
                CustomValue::Ints(v)
            }
            'l' => CustomValue::Ints(vec![self.i64()?]),
            'd' => CustomValue::Ints(vec![self.i32()? as i64]),
            'b' => CustomValue::Bytes(self.take(count)?.to_vec()),
            _ => return Err(self.err(format!("unknown custom struct kind {kind}"))),
        })
    }

    fn read_text(&mut self) -> Result<TextValue, Error> {
        let flags = self.u32()?;
        let history = self.i8()?;
        let mut t = TextValue {
            flags,
            history,
            namespace: None,
            key: None,
            source: None,
            table: None,
        };
        match history {
            0 => {
                // Base: namespace, key, source string
                t.namespace = Some(self.fstring()?);
                t.key = Some(self.fstring()?);
                t.source = Some(self.fstring()?);
            }
            -1 => {
                // None: optional culture-invariant string
                let has = self.i32()?;
                t.source = if has == 1 {
                    Some(self.fstring()?)
                } else {
                    None
                };
            }
            12 => {
                // StringTableEntry
                t.table = Some(self.fname()?);
                t.key = Some(self.fstring()?);
            }
            other => return Err(self.err(format!("unhandled TextHistoryType {other}"))),
        }
        Ok(t)
    }

    /// Value-only read for array items (no tag, no GUID flag). Returns the
    /// value and the span a same-size splice may overwrite (for strings this
    /// excludes the length prefix).
    fn array_item(&mut self, t: &str) -> Result<(PropValue, usize, usize), Error> {
        let start = self.pos;
        let v = match t {
            "StrProperty" => {
                let n = self.i32()?;
                let vstart = self.pos;
                let s = self.fstring_body(n)?;
                return Ok((PropValue::Str(s), vstart, self.pos));
            }
            "IntProperty" => PropValue::Int(self.i32()?),
            "FloatProperty" => PropValue::Float(self.f32()?),
            "BoolProperty" => {
                let b = self.u8()?;
                if b > 1 {
                    return Err(self.err(format!("invalid bool byte {b} in array item")));
                }
                PropValue::Bool(b == 1)
            }
            "NameProperty" => PropValue::Name(self.fname()?),
            "ObjectProperty" => PropValue::Object(self.i32()?),
            "ByteProperty" => PropValue::Byte {
                enum_name: String::new(),
                value: ByteValue::Byte(self.u8()?),
            },
            "TextProperty" => PropValue::Text(self.read_text()?),
            other => return Err(self.err(format!("unsupported array item type {other}"))),
        };
        Ok((v, start, self.pos))
    }

    fn prop_list(&mut self, ctx: &str) -> Result<Vec<Prop>, Error> {
        let mut props = Vec::new();
        loop {
            let start = self.pos;
            let nm = self.fname()?;
            if nm.value == "None" {
                return Ok(props);
            }
            let ty = self.fname()?;
            let size = self.i32()?;
            let array_index = self.i32()?;

            let mut p = Prop {
                name: nm.value,
                type_name: ty.value,
                size,
                array_index,
                start,
                vstart: 0,
                vend: 0,
                struct_type: None,
                custom: None,
                value: PropValue::Struct {
                    children: Vec::new(),
                },
            };
            self.tick("props_total");
            self.tick(&format!("prop:{}", p.type_name));

            match p.type_name.as_str() {
                "StructProperty" => {
                    let st = self.fname()?;
                    p.struct_type = Some(st.value.clone());
                    self.skip(16)?; // struct GUID
                    self.skip_guid_flag()?;
                    let vstart = self.pos;
                    match custom_kind(&st.value) {
                        Some((k, n)) => {
                            p.custom = Some(st.value.clone());
                            self.tick(&format!("customstruct:{}", st.value));
                            p.value = PropValue::Custom(self.read_custom(k, n)?);
                        }
                        None => {
                            let children = if size == 0 {
                                Vec::new()
                            } else {
                                self.prop_list(&format!("{ctx}>struct"))?
                            };
                            p.value = PropValue::Struct { children };
                        }
                    }
                    p.vstart = vstart;
                    p.vend = self.pos;
                    if size as usize != self.pos - vstart {
                        self.mismatches.push(format!(
                            "{ctx}/{}: struct {} size={} consumed={}",
                            p.name,
                            st.value,
                            size,
                            self.pos - vstart
                        ));
                    }
                }
                "BoolProperty" => {
                    let b = self.u8()?;
                    if b > 1 {
                        return Err(self.err(format!("invalid bool byte {b} on {}", p.name)));
                    }
                    self.skip_guid_flag()?;
                    p.value = PropValue::Bool(b == 1);
                    // the value byte lives inside the tag, right after name/type/size/arrayIndex
                    p.vstart = start + 24;
                    p.vend = start + 25;
                    if size != 0 {
                        self.mismatches
                            .push(format!("{ctx}/{}: bool size={} (expected 0)", p.name, size));
                    }
                }
                "ByteProperty" => {
                    let et = self.fname()?;
                    self.skip_guid_flag()?;
                    let vstart = self.pos;
                    let value = match size {
                        1 => ByteValue::Byte(self.u8()?),
                        8 => ByteValue::FName(self.fname()?),
                        other => {
                            return Err(self.err(format!("ByteProperty size={other} on {}", p.name)))
                        }
                    };
                    p.value = PropValue::Byte {
                        enum_name: et.value,
                        value,
                    };
                    p.vstart = vstart;
                    p.vend = self.pos;
                }
                "ArrayProperty" => {
                    let at = self.fname()?;
                    self.skip_guid_flag()?;
                    let vstart = self.pos;
                    let count = self.i32()?;
                    let mut items = Vec::new();
                    if at.value == "StructProperty" {
                        let nm2 = self.fname()?;
                        if nm2.value != "None" {
                            let _tat = self.fname()?;
                            let _length = self.i64()?;
                            let full = self.fname()?;
                            self.skip(16)?;
                            self.skip_guid_flag()?;
                            for _ in 0..count {
                                let item_start = self.pos;
                                match custom_kind(&full.value) {
                                    Some((k, n)) => {
                                        self.tick(&format!("customstruct:{}", full.value));
                                        let value = self.read_custom(k, n)?;
                                        items.push(ArrayElem::Custom {
                                            kind: full.value.clone(),
                                            value,
                                            start: item_start,
                                            end: self.pos,
                                        });
                                    }
                                    None => items.push(ArrayElem::Struct(
                                        self.prop_list(&format!("{ctx}>arr-struct"))?,
                                    )),
                                }
                            }
                        }
                    } else {
                        for _ in 0..count {
                            let (value, start, end) = self.array_item(&at.value)?;
                            items.push(ArrayElem::Prim { value, start, end });
                        }
                    }
                    p.value = PropValue::Array {
                        elem_type: at.value,
                        count,
                        items,
                    };
                    p.vstart = vstart;
                    p.vend = self.pos;
                    if size as usize != self.pos - vstart {
                        self.mismatches.push(format!(
                            "{ctx}/{}: array size={} consumed={}",
                            p.name,
                            size,
                            self.pos - vstart
                        ));
                    }
                }
                "TextProperty" => {
                    self.skip_guid_flag()?;
                    let vstart = self.pos;
                    p.value = PropValue::Text(self.read_text()?);
                    p.vstart = vstart;
                    p.vend = self.pos;
                    if size as usize != self.pos - vstart {
                        self.mismatches.push(format!(
                            "{ctx}/{}: text size={} consumed={}",
                            p.name,
                            size,
                            self.pos - vstart
                        ));
                    }
                }
                "StrProperty" => {
                    self.skip_guid_flag()?;
                    let full_start = self.pos;
                    let len = self.i32()?;
                    let vstart = self.pos;
                    p.value = PropValue::Str(self.fstring_body(len)?);
                    p.vstart = vstart;
                    p.vend = self.pos;
                    if size as usize != self.pos - full_start {
                        self.mismatches.push(format!(
                            "{ctx}/{}: str size={} consumed={}",
                            p.name,
                            size,
                            self.pos - full_start
                        ));
                    }
                }
                "IntProperty" => {
                    self.skip_guid_flag()?;
                    let vstart = self.pos;
                    p.value = PropValue::Int(self.i32()?);
                    p.vstart = vstart;
                    p.vend = self.pos;
                    if size != 4 {
                        self.mismatches
                            .push(format!("{ctx}/{}: int size={}", p.name, size));
                    }
                }
                "FloatProperty" => {
                    self.skip_guid_flag()?;
                    let vstart = self.pos;
                    p.value = PropValue::Float(self.f32()?);
                    p.vstart = vstart;
                    p.vend = self.pos;
                    if size != 4 {
                        self.mismatches
                            .push(format!("{ctx}/{}: float size={}", p.name, size));
                    }
                }
                "NameProperty" => {
                    self.skip_guid_flag()?;
                    let vstart = self.pos;
                    p.value = PropValue::Name(self.fname()?);
                    p.vstart = vstart;
                    p.vend = self.pos;
                    if size != 8 {
                        self.mismatches
                            .push(format!("{ctx}/{}: name size={}", p.name, size));
                    }
                }
                "ObjectProperty" => {
                    self.skip_guid_flag()?;
                    let vstart = self.pos;
                    p.value = PropValue::Object(self.i32()?);
                    p.vstart = vstart;
                    p.vend = self.pos;
                    if size != 4 {
                        self.mismatches
                            .push(format!("{ctx}/{}: object size={}", p.name, size));
                    }
                }
                other => {
                    return Err(
                        self.err(format!("unsupported property type {other} for {}", p.name))
                    )
                }
            }

            props.push(p);
        }
    }
}
