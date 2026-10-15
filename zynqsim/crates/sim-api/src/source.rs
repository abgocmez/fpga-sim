//! Source-level debug information: the DWARF line table of an ELF.
//!
//! [`SourceMap`] answers "which `file:line:column` and function is this
//! address?" and the reverse "which addresses does `file.rs:42` start at?".
//! It reads `.debug_line` (and the subprogram DIEs of `.debug_info` for
//! function names) with the pure-Rust `gimli` and `object` crates. An ELF
//! built without debug info gives an empty map, never an error.
//!
//! Only what the line table says is reported: no variables, no inlined-call
//! chains, no types. See `docs/adr/013-dwarf-source.md`.

use crate::error::ApiError;
use gimli::{EndianSlice, LittleEndian};
use object::{Object, ObjectSection};
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Largest source file `read_source` returns.
pub const MAX_SOURCE_BYTES: u64 = 2 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileEntry {
    /// The path as the line table names it, `/` separators, made relative to
    /// the compilation directory when it lies inside it.
    path: String,
    /// The compilation directory of the unit that referenced it.
    comp_dir: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct Row {
    addr: u32,
    file: u32,
    /// 0 marks the end of a sequence (no code from here on).
    line: u32,
    column: u32,
    stmt: bool,
}

#[derive(Clone, Debug)]
struct Func {
    low: u32,
    high: u32,
    name: String,
}

/// A resolved code address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located<'a> {
    /// Source file as named in the line table.
    pub file: &'a str,
    /// 1-based line.
    pub line: u32,
    /// 1-based column, 0 when the compiler did not say.
    pub column: u32,
    /// Function (DWARF `DW_AT_name`) containing the address, when known.
    pub function: Option<&'a str>,
}

/// Where a `file:line` request landed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineAddrs {
    /// File as named in the line table.
    pub file: String,
    /// The line that has code: the requested one, or the next one after it
    /// that does (a comment or a blank line has none).
    pub line: u32,
    /// Where the line's code starts (one address per block of the line, as a
    /// loop or a generic function instantiated twice gives several).
    pub addrs: Vec<u32>,
}

/// The line table and function ranges of one ELF, sorted by address.
#[derive(Clone, Debug, Default)]
pub struct SourceMap {
    files: Vec<FileEntry>,
    rows: Vec<Row>,
    funcs: Vec<Func>,
}

fn normalize(p: &str) -> String {
    p.replace('\\', "/")
}

fn is_absolute(p: &str) -> bool {
    p.starts_with('/') || (p.len() >= 2 && p.as_bytes()[1] == b':')
}

/// `a/b/c` -> true for suffix `b/c` (component-wise, ASCII case-insensitive:
/// the host may be Windows and DWARF paths come from the build machine).
fn has_suffix(path: &str, suffix: &str) -> bool {
    let p: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let s: Vec<&str> = suffix
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    if s.is_empty() || s.len() > p.len() {
        return false;
    }
    p[p.len() - s.len()..]
        .iter()
        .zip(&s)
        .all(|(a, b)| a.eq_ignore_ascii_case(b))
}

impl SourceMap {
    /// Reads the DWARF of the ELF at `path`.
    pub fn load(path: &Path) -> Result<SourceMap, ApiError> {
        let d = std::fs::read(path)
            .map_err(|e| ApiError::io(format!("cannot read {}: {e}", path.display())))?;
        SourceMap::parse(&d).map_err(|m| {
            ApiError::invalid_argument(format!("{}: cannot read debug info: {m}", path.display()))
        })
    }

    /// Parses ELF bytes. No `.debug_line` section gives an empty map.
    pub fn parse(data: &[u8]) -> Result<SourceMap, String> {
        let obj = object::File::parse(data).map_err(|e| e.to_string())?;
        let sections = gimli::DwarfSections::load(|id| -> Result<Cow<[u8]>, object::Error> {
            Ok(match obj.section_by_name(id.name()) {
                Some(s) => s.uncompressed_data()?,
                None => Cow::Borrowed(&[]),
            })
        })
        .map_err(|e| e.to_string())?;
        let dwarf = sections.borrow(|s| EndianSlice::new(s, LittleEndian));

        let mut map = SourceMap::default();
        let mut file_ids: HashMap<(String, Option<String>), u32> = HashMap::new();
        let mut rows: Vec<Row> = Vec::new();
        let mut units = dwarf.units();
        while let Some(header) = units.next().map_err(|e| e.to_string())? {
            let unit = dwarf.unit(header).map_err(|e| e.to_string())?;
            let comp_dir = unit
                .comp_dir
                .as_ref()
                .map(|d| normalize(&d.to_string_lossy()));
            // Function ranges.
            {
                let mut entries = unit.entries();
                while let Some(e) = entries.next_dfs().map_err(|e| e.to_string())? {
                    if e.tag() != gimli::DW_TAG_subprogram {
                        continue;
                    }
                    let name = Self::die_name(&dwarf, &unit, e);
                    let Some(name) = name else { continue };
                    let Ok(mut ranges) = dwarf.die_ranges(&unit, e) else {
                        continue;
                    };
                    while let Ok(Some(r)) = ranges.next() {
                        if r.begin != 0 && r.end > r.begin && r.end <= u64::from(u32::MAX) {
                            map.funcs.push(Func {
                                low: r.begin as u32,
                                high: r.end as u32,
                                name: name.clone(),
                            });
                        }
                    }
                }
            }
            // Line rows.
            let Some(program) = unit.line_program.clone() else {
                continue;
            };
            let mut it = program.rows();
            // Sequences starting at address 0 belong to code the linker
            // discarded; skip them whole.
            let mut skip = false;
            let mut first_in_seq = true;
            while let Some((hdr, row)) = it.next_row().map_err(|e| e.to_string())? {
                if first_in_seq {
                    skip = row.address() == 0;
                    first_in_seq = false;
                }
                if row.end_sequence() {
                    if !skip && row.address() <= u64::from(u32::MAX) {
                        rows.push(Row {
                            addr: row.address() as u32,
                            file: 0,
                            line: 0,
                            column: 0,
                            stmt: false,
                        });
                    }
                    first_in_seq = true;
                    continue;
                }
                if skip || row.address() > u64::from(u32::MAX) {
                    continue;
                }
                let Some(entry) = row.file(hdr) else { continue };
                let name = match dwarf.attr_string(&unit, entry.path_name()) {
                    Ok(n) => normalize(&n.to_string_lossy()),
                    Err(_) => continue,
                };
                let dir = entry
                    .directory(hdr)
                    .and_then(|d| dwarf.attr_string(&unit, d).ok())
                    .map(|d| normalize(&d.to_string_lossy()));
                let mut path = match dir {
                    Some(d) if !d.is_empty() && !is_absolute(&name) => format!("{d}/{name}"),
                    _ => name,
                };
                if let Some(cd) = &comp_dir {
                    if let Some(rest) = path.strip_prefix(&format!("{cd}/")) {
                        path = rest.to_string();
                    }
                }
                let key = (path, comp_dir.clone());
                let next = map.files.len() as u32;
                let id = *file_ids.entry(key.clone()).or_insert_with(|| {
                    map.files.push(FileEntry {
                        path: key.0.clone(),
                        comp_dir: key.1.clone(),
                    });
                    next
                });
                let line = row.line().map_or(0, |l| l.get() as u32);
                if line == 0 {
                    continue; // compiler-generated, no source line
                }
                let column = match row.column() {
                    gimli::ColumnType::LeftEdge => 0,
                    gimli::ColumnType::Column(c) => c.get() as u32,
                };
                rows.push(Row {
                    addr: row.address() as u32,
                    file: id,
                    line,
                    column,
                    stmt: row.is_stmt(),
                });
            }
        }
        // End-of-sequence rows sort before the first row of an adjacent
        // sequence at the same address; otherwise stable order is kept.
        rows.sort_by_key(|r| (r.addr, u8::from(r.line != 0)));
        map.rows = rows;
        map.funcs.sort_by_key(|f| (f.low, f.high));
        map.funcs
            .dedup_by(|a, b| a.low == b.low && a.high == b.high);
        Ok(map)
    }

    fn die_name<'a>(
        dwarf: &gimli::Dwarf<EndianSlice<'a, LittleEndian>>,
        unit: &gimli::Unit<EndianSlice<'a, LittleEndian>>,
        e: &gimli::DebuggingInformationEntry<EndianSlice<'a, LittleEndian>>,
    ) -> Option<String> {
        let mut cur = e.clone();
        for _ in 0..3 {
            if let Some(v) = cur.attr_value(gimli::DW_AT_name) {
                if let Ok(s) = dwarf.attr_string(unit, v) {
                    return Some(s.to_string_lossy().into_owned());
                }
            }
            let origin = cur
                .attr_value(gimli::DW_AT_abstract_origin)
                .or_else(|| cur.attr_value(gimli::DW_AT_specification));
            match origin {
                Some(gimli::AttributeValue::UnitRef(off)) => {
                    cur = unit.entry(off).ok()?;
                }
                _ => return None,
            }
        }
        None
    }

    /// True when the ELF carried no line information.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Number of line-table rows kept.
    #[cfg(test)]
    pub fn row_count(&self) -> usize {
        self.rows.iter().filter(|r| r.line != 0).count()
    }

    /// The source files of the line table, as named there, sorted.
    pub fn files(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.files.iter().map(|f| f.path.as_str()).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The function (DWARF name) that contains `addr`.
    pub fn function_at(&self, addr: u32) -> Option<&str> {
        let i = self.funcs.partition_point(|f| f.low <= addr);
        // Nested/overlapping ranges are rare; look back a few entries and
        // take the innermost (the one starting latest).
        self.funcs[..i]
            .iter()
            .rev()
            .take(16)
            .find(|f| addr < f.high)
            .map(|f| f.name.as_str())
    }

    /// File, line and column of the instruction at `addr`. `None` when no
    /// code is known there (outside every sequence).
    pub fn lookup(&self, addr: u32) -> Option<Located<'_>> {
        let i = self.rows.partition_point(|r| r.addr <= addr);
        let r = self.rows[..i].last()?;
        if r.line == 0 {
            return None;
        }
        Some(Located {
            file: &self.files[r.file as usize].path,
            line: r.line,
            column: r.column,
            function: self.function_at(addr),
        })
    }

    /// Indices of the files whose path ends with `file` (component-wise).
    fn match_files(&self, file: &str) -> Vec<u32> {
        let f = normalize(file);
        let f = f.trim();
        let exact: Vec<u32> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, e)| e.path == f)
            .map(|(i, _)| i as u32)
            .collect();
        if !exact.is_empty() {
            return exact;
        }
        self.files
            .iter()
            .enumerate()
            .filter(|(_, e)| has_suffix(&e.path, f))
            .map(|(i, _)| i as u32)
            .collect()
    }

    /// Where `file:line` has code. `file` is matched by trailing path
    /// components (`math.rs`, `src/main.rs`), so a bare file name works. A line
    /// without code resolves to the next line of that file that has some.
    pub fn addrs_for(&self, file: &str, line: u32) -> Result<LineAddrs, String> {
        let ids = self.match_files(file);
        if ids.is_empty() {
            let known: Vec<&str> = self.files();
            return Err(if known.is_empty() {
                "the ELF has no line information (build it with debug info, for example \
                 `debug = \"line-tables-only\"`)"
                    .to_string()
            } else {
                let shown: Vec<&str> = known.iter().copied().take(8).collect();
                format!(
                    "no source file `{file}` in the line table (known: {}{})",
                    shown.join(", "),
                    if known.len() > shown.len() {
                        ", ..."
                    } else {
                        ""
                    }
                )
            });
        }
        let mut file_names: Vec<&str> = ids
            .iter()
            .map(|i| self.files[*i as usize].path.as_str())
            .collect();
        file_names.sort_unstable();
        file_names.dedup();
        if file_names.len() > 1 {
            return Err(format!(
                "`{file}` is ambiguous: {} (give more of the path)",
                file_names.join(", ")
            ));
        }
        // Smallest line >= requested that has an is_stmt row.
        let best = self
            .rows
            .iter()
            .filter(|r| r.line >= line && ids.contains(&r.file) && r.stmt)
            .map(|r| r.line)
            .min()
            .ok_or_else(|| format!("{}:{line} is past the last line with code", file_names[0]))?;
        // One address per contiguous run of rows of that line: the first row
        // of each run.
        let mut addrs: Vec<u32> = Vec::new();
        let mut prev_matches = false;
        for r in &self.rows {
            let m = r.line == best && ids.contains(&r.file);
            if m && r.stmt && !prev_matches {
                addrs.push(r.addr);
            }
            prev_matches = m;
        }
        addrs.sort_unstable();
        addrs.dedup();
        // Prefer the address after the prologue when the line is the function's
        // first: the line table's own first row for the function is fine, so
        // no skipping (matches `break main` landing on the symbol address).
        addrs.truncate(16);
        Ok(LineAddrs {
            file: file_names[0].to_string(),
            line: best,
            addrs,
        })
    }

    /// Candidate host paths for a line-table file, most specific first:
    /// absolute names, the compilation directory, then each search directory
    /// (also with leading components dropped, down to two).
    pub fn candidates(&self, file: &str, search: &[PathBuf]) -> Vec<PathBuf> {
        let f = normalize(file);
        let mut out: Vec<PathBuf> = Vec::new();
        if is_absolute(&f) {
            out.push(PathBuf::from(&f));
        } else {
            let mut seen: Vec<&str> = Vec::new();
            for e in self.files.iter().filter(|e| e.path == f) {
                // A relative compilation directory (remapped at build time) is
                // meaningless here; the search directories stand in for it.
                if let Some(cd) = e.comp_dir.as_ref().filter(|c| is_absolute(c)) {
                    if !seen.contains(&cd.as_str()) {
                        seen.push(cd);
                        out.push(Path::new(cd).join(&f));
                    }
                }
            }
            for d in search {
                out.push(d.join(&f));
            }
            let comps: Vec<&str> = f.split('/').filter(|c| !c.is_empty()).collect();
            for skip in 1..comps.len().saturating_sub(1) {
                let rest = comps[skip..].join("/");
                for d in search {
                    out.push(d.join(&rest));
                }
            }
        }
        out
    }

    /// Finds the host file for a line-table `file` and reads it as text.
    /// Only files the line table names can be read (this is a debugger
    /// feature, not a file server). Returns the path that was read and its
    /// text, or why it could not be found.
    pub fn read_source(&self, file: &str, search: &[PathBuf]) -> Result<(PathBuf, String), String> {
        let ids = self.match_files(file);
        let Some(first) = ids.first() else {
            return Err(format!("`{file}` is not a source file of this ELF"));
        };
        let name = self.files[*first as usize].path.clone();
        let cands = self.candidates(&name, search);
        for c in &cands {
            let Ok(meta) = std::fs::metadata(c) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if meta.len() > MAX_SOURCE_BYTES {
                return Err(format!("{} is larger than 2 MiB", c.display()));
            }
            return match std::fs::read(c) {
                Ok(b) => Ok((c.clone(), String::from_utf8_lossy(&b).into_owned())),
                Err(e) => Err(format!("cannot read {}: {e}", c.display())),
            };
        }
        let tried: Vec<String> = cands
            .iter()
            .take(6)
            .map(|c| c.display().to_string())
            .collect();
        Err(format!(
            "source file `{name}` not found on this machine (tried {}); add its directory \
             to `[debug] source_dirs` in project.toml",
            tried.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(n: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/elf")
            .join(n)
    }

    #[test]
    fn suffix_matching_is_by_component() {
        assert!(has_suffix("a/b/src/main.rs", "main.rs"));
        assert!(has_suffix("a/b/src/main.rs", "src/main.rs"));
        assert!(has_suffix("A/B/SRC/Main.rs", "src/main.rs"));
        assert!(!has_suffix("a/b/domain.rs", "main.rs"));
        assert!(!has_suffix("main.rs", "a/main.rs"));
    }

    #[test]
    fn an_elf_without_debug_info_gives_an_empty_map() {
        let m = SourceMap::load(&fixture("rust_hello.elf")).unwrap();
        assert!(m.is_empty());
        assert!(m.lookup(0x0010_0000).is_none());
        let e = m.addrs_for("main.rs", 1).unwrap_err();
        assert!(e.contains("no line information"), "{e}");
    }

    // ---- a tiny ELF whose line table the test writes by hand ----

    use gimli::write::{
        Address, AttributeValue, Dwarf, EndianVec, LineProgram, LineString, Sections, Unit,
    };

    /// One unit: `(file, dir)`, sequences `(start, [(offset, line, column, stmt)], end offset)`
    /// and functions `(name, low, size)`.
    /// `(offset, line, column, is_stmt)`.
    type TestRow = (u64, u64, u64, bool);

    struct TestUnit {
        comp_dir: &'static str,
        dir: &'static str,
        file: &'static str,
        sequences: Vec<(u64, Vec<TestRow>, u64)>,
        funcs: Vec<(&'static str, u64, u64)>,
    }

    fn build_elf(units: &[TestUnit]) -> Vec<u8> {
        let enc = gimli::Encoding {
            format: gimli::Format::Dwarf32,
            version: 4,
            address_size: 4,
        };
        let mut dwarf = Dwarf::new();
        for u in units {
            let mut lp = LineProgram::new(
                enc,
                gimli::LineEncoding::default(),
                LineString::String(u.comp_dir.as_bytes().to_vec()),
                None,
                LineString::String(u.file.as_bytes().to_vec()),
                None,
            );
            let dir = if u.dir.is_empty() {
                lp.default_directory()
            } else {
                lp.add_directory(LineString::String(u.dir.as_bytes().to_vec()))
            };
            let file = lp.add_file(LineString::String(u.file.as_bytes().to_vec()), dir, None);
            for (start, rows, end) in &u.sequences {
                lp.begin_sequence(Some(Address::Constant(*start)));
                for (off, line, col, stmt) in rows {
                    let r = lp.row();
                    r.address_offset = *off;
                    r.file = file;
                    r.line = *line;
                    r.column = *col;
                    r.is_statement = *stmt;
                    lp.generate_row();
                }
                lp.end_sequence(*end);
            }
            let id = dwarf.units.add(Unit::new(enc, lp));
            let unit = dwarf.units.get_mut(id);
            let root = unit.root();
            unit.get_mut(root).set(
                gimli::DW_AT_comp_dir,
                AttributeValue::String(u.comp_dir.as_bytes().to_vec()),
            );
            unit.get_mut(root).set(
                gimli::DW_AT_name,
                AttributeValue::String(u.file.as_bytes().to_vec()),
            );
            for (name, low, size) in &u.funcs {
                let f = unit.add(root, gimli::DW_TAG_subprogram);
                let e = unit.get_mut(f);
                e.set(
                    gimli::DW_AT_name,
                    AttributeValue::String(name.as_bytes().to_vec()),
                );
                e.set(
                    gimli::DW_AT_low_pc,
                    AttributeValue::Address(Address::Constant(*low)),
                );
                e.set(gimli::DW_AT_high_pc, AttributeValue::Udata(*size));
            }
        }
        let mut sections = Sections::new(EndianVec::new(gimli::LittleEndian));
        dwarf.write(&mut sections).unwrap();
        let mut obj = object::write::Object::new(
            object::BinaryFormat::Elf,
            object::Architecture::Arm,
            object::Endianness::Little,
        );
        sections
            .for_each(|id, w| -> Result<(), ()> {
                if !w.slice().is_empty() {
                    let s = obj.add_section(
                        Vec::new(),
                        id.name().as_bytes().to_vec(),
                        object::SectionKind::Debug,
                    );
                    obj.set_section_data(s, w.slice().to_vec(), 1);
                }
                Ok(())
            })
            .unwrap();
        obj.write().unwrap()
    }

    fn tiny() -> SourceMap {
        let elf = build_elf(&[
            TestUnit {
                comp_dir: "/build/proj",
                dir: "src",
                file: "alpha.rs",
                // Line 10 appears twice (a loop): two blocks of that line.
                sequences: vec![(
                    0x10_0000,
                    vec![
                        (0, 10, 5, true),
                        (8, 11, 9, true),
                        (16, 10, 5, true),
                        (24, 12, 1, false),
                    ],
                    32,
                )],
                funcs: vec![("alpha", 0x10_0000, 32)],
            },
            // Starts exactly where the first sequence ends.
            TestUnit {
                comp_dir: "/build/proj",
                dir: "",
                file: "beta.rs",
                sequences: vec![(0x10_0020, vec![(0, 5, 3, true), (4, 6, 3, true)], 12)],
                funcs: vec![("beta", 0x10_0020, 12)],
            },
            // Code the linker threw away is addressed from 0: must be ignored.
            TestUnit {
                comp_dir: "/build/proj",
                dir: "",
                file: "gone.rs",
                sequences: vec![(0, vec![(0, 1, 1, true), (4, 2, 1, true)], 8)],
                funcs: vec![],
            },
        ]);
        SourceMap::parse(&elf).unwrap()
    }

    #[test]
    fn addresses_map_to_file_line_column_and_function() {
        let m = tiny();
        assert!(!m.is_empty());
        let at = |a: u32| m.lookup(a).map(|l| (l.file.to_string(), l.line, l.column));
        assert_eq!(at(0x10_0000), Some(("src/alpha.rs".into(), 10, 5)));
        assert_eq!(at(0x10_0007), Some(("src/alpha.rs".into(), 10, 5)));
        assert_eq!(at(0x10_0008), Some(("src/alpha.rs".into(), 11, 9)));
        assert_eq!(at(0x10_0010), Some(("src/alpha.rs".into(), 10, 5)));
        assert_eq!(at(0x10_001f), Some(("src/alpha.rs".into(), 12, 1)));
        // The next sequence begins where the first ended: no gap, no leak.
        assert_eq!(at(0x10_0020), Some(("beta.rs".into(), 5, 3)));
        assert_eq!(at(0x10_0024), Some(("beta.rs".into(), 6, 3)));
        // Past the end, before the start, and in discarded code: nothing.
        assert_eq!(at(0x10_002c), None);
        assert_eq!(at(0x0f_ffff), None);
        assert_eq!(at(0x4), None);
        assert_eq!(m.lookup(0x10_0008).unwrap().function, Some("alpha"));
        assert_eq!(m.lookup(0x10_0024).unwrap().function, Some("beta"));
        assert_eq!(m.function_at(0x10_002c), None);
        assert_eq!(m.row_count(), 6);
        assert_eq!(m.files(), vec!["beta.rs", "src/alpha.rs"]);
    }

    #[test]
    fn a_source_line_maps_back_to_the_start_of_each_block() {
        let m = tiny();
        // A trailing part of the path is enough.
        let l = m.addrs_for("alpha.rs", 10).unwrap();
        assert_eq!((l.file.as_str(), l.line), ("src/alpha.rs", 10));
        assert_eq!(l.addrs, vec![0x10_0000, 0x10_0010]);
        assert_eq!(
            m.addrs_for("src/alpha.rs", 11).unwrap().addrs,
            vec![0x10_0008]
        );
        assert_eq!(
            m.addrs_for("SRC\\ALPHA.RS", 11).unwrap().addrs,
            vec![0x10_0008]
        );
        // Line 12 is not a statement, line 9 has no code: both resolve forward
        // to the next line that does (10), or fail when none is left.
        assert_eq!(m.addrs_for("alpha.rs", 9).unwrap().line, 10);
        assert!(
            m.addrs_for("alpha.rs", 12)
                .unwrap_err()
                .contains("past the last")
        );
        let e = m.addrs_for("nosuch.rs", 1).unwrap_err();
        assert!(
            e.contains("no source file") && e.contains("alpha.rs"),
            "{e}"
        );
        // `a.rs` must not match `alpha.rs`.
        assert!(m.addrs_for("a.rs", 10).is_err());
        // Every address maps back to the line it was asked for.
        for a in &l.addrs {
            assert_eq!(m.lookup(*a).unwrap().line, 10);
        }
    }

    #[test]
    fn ambiguous_file_names_are_refused() {
        let unit = |dir: &'static str| TestUnit {
            comp_dir: "/b",
            dir,
            file: "lib.rs",
            sequences: vec![(0x2000, vec![(0, 1, 1, true)], 4)],
            funcs: vec![],
        };
        let m = SourceMap::parse(&build_elf(&[unit("one"), unit("two")])).unwrap();
        let e = m.addrs_for("lib.rs", 1).unwrap_err();
        assert!(e.contains("ambiguous"), "{e}");
        assert!(m.addrs_for("one/lib.rs", 1).is_ok());
    }

    #[test]
    fn source_files_are_found_through_the_search_directories_only_if_named() {
        let m = tiny();
        let dir = std::env::temp_dir().join(format!("zynqsim-src-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/alpha.rs"), "fn alpha() {}\n").unwrap();
        // A file that exists but is not in the line table is never served.
        std::fs::write(dir.join("secret.txt"), "no").unwrap();

        let (p, text) = m
            .read_source("alpha.rs", std::slice::from_ref(&dir))
            .unwrap();
        assert!(p.ends_with("alpha.rs"));
        assert_eq!(text, "fn alpha() {}\n");
        let e = m
            .read_source("secret.txt", std::slice::from_ref(&dir))
            .unwrap_err();
        assert!(e.contains("not a source file"), "{e}");
        // beta.rs is in the line table but not on disk: say where it looked.
        let e = m
            .read_source("beta.rs", std::slice::from_ref(&dir))
            .unwrap_err();
        assert!(e.contains("source_dirs") && e.contains("beta.rs"), "{e}");
        // Without search directories only the absolute compilation directory
        // is tried.
        assert!(m.read_source("alpha.rs", &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_rust_debug_demo_maps_its_marked_lines_both_ways() {
        let m = SourceMap::load(&fixture("rust_debug.elf")).unwrap();
        assert!(!m.is_empty(), "rust_debug.elf must carry line tables");
        let src = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../firmware/debug_demo/src/main.rs"),
        )
        .unwrap();
        let line_of = |mark: &str| {
            src.lines()
                .position(|l| l.contains(&format!("MARK:{mark}")))
                .map(|i| i as u32 + 1)
                .unwrap_or_else(|| panic!("no MARK:{mark}"))
        };
        for (mark, func) in [
            ("collatz-step", "collatz_steps"),
            ("call-collatz", "main"),
            ("call-checksum", "main"),
        ] {
            let line = line_of(mark);
            let found = m.addrs_for("main.rs", line).unwrap();
            assert_eq!(found.line, line, "{mark} has code on its own line");
            assert!(
                found.file.ends_with("debug_demo/src/main.rs"),
                "{}",
                found.file
            );
            assert!(!found.addrs.is_empty());
            for a in &found.addrs {
                let l = m.lookup(*a).unwrap();
                assert_eq!((l.line, l.file), (line, found.file.as_str()), "{mark}");
                assert_eq!(l.function, Some(func), "{mark}");
            }
        }
        // The compiled-in core library is there too, so a stop inside
        // `println!` machinery also has a line (its source is not on disk).
        assert!(m.files().iter().any(|f| f.starts_with("library/core/")));
    }

    #[test]
    fn non_elf_input_is_rejected() {
        assert!(SourceMap::parse(b"hello").is_err());
    }
}
