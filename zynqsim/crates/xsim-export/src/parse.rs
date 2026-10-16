//! The readers for each list format and the accumulator they share.

use crate::lex::{self, Tok};
use crate::walk;
use crate::{Diagnostic, ExportDesign, Lang, Origin, Severity, SourceFile};
use std::path::{Path, PathBuf};

type EnvFn = Box<dyn Fn(&str) -> Option<String>>;

/// Reads exports. Holds how variables (`$XILINX_VIVADO`, `%XILINX_VIVADO%`) are looked up.
pub struct Reader {
    env: EnvFn,
}

impl Default for Reader {
    fn default() -> Self {
        Self::new()
    }
}

/// How deep `-f` / `-prj` lists may include each other.
const MAX_NESTING: usize = 8;

impl Reader {
    /// A reader that looks variables up in the process environment.
    pub fn new() -> Reader {
        Reader {
            env: Box::new(|n| std::env::var(n).ok()),
        }
    }

    /// A reader with its own variable lookup (tests, or a known Vivado install).
    pub fn with_env(f: impl Fn(&str) -> Option<String> + 'static) -> Reader {
        Reader { env: Box::new(f) }
    }

    /// Reads a folder or a single list/script/option file.
    pub fn read(&self, path: &Path) -> Result<ExportDesign, String> {
        if !path.exists() {
            return Err(format!("{} does not exist", path.display()));
        }
        let mut acc = Acc {
            r: self,
            d: ExportDesign::default(),
            seen: Vec::new(),
            vars: Vec::new(),
            expect_sources: false,
        };
        let p = lex::resolve(Path::new(""), &path.display().to_string());
        acc.any(&p, 0)?;
        acc.finish();
        Ok(acc.d)
    }
}

struct Acc<'r> {
    r: &'r Reader,
    d: ExportDesign,
    seen: Vec<PathBuf>,
    /// Shell variables of the script being read (`xvlog_opts=...`).
    vars: Vec<(String, String)>,
    /// The input is a list or a folder, so finding no source is an error.
    expect_sources: bool,
}

/// What the option scanner found in one command or list line.
#[derive(Default)]
struct Scan {
    includes: Vec<String>,
    defines: Vec<String>,
    libs: Vec<String>,
    work: Option<String>,
    sv: bool,
    vhdl2008: bool,
    relax: bool,
    /// `-f FILE` / `-prj FILE`
    nested: Vec<String>,
    generics: Vec<String>,
    timescale: Option<String>,
    top: Vec<String>,
    positional: Vec<Tok>,
    /// Options this reader does not know: (text, reason).
    unknown: Vec<(String, String)>,
}

/// Options that take a value and are understood (the second field is what they mean).
fn scan(toks: &[Tok]) -> Scan {
    let mut s = Scan::default();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let text = t.text.as_str();
        i += 1;
        if t.quoted || !(text.starts_with('-') || text.starts_with('+')) {
            s.positional.push(t.clone());
            continue;
        }
        // `+incdir+a+b`, `+define+A=1+B`
        if let Some(rest) = text.strip_prefix("+incdir+") {
            s.includes
                .extend(rest.split('+').filter(|x| !x.is_empty()).map(String::from));
            continue;
        }
        if let Some(rest) = text.strip_prefix("+define+") {
            s.defines
                .extend(rest.split('+').filter(|x| !x.is_empty()).map(String::from));
            continue;
        }
        if text.starts_with('+') {
            s.unknown.push((
                text.to_string(),
                "plus option is not supported and is ignored".to_string(),
            ));
            continue;
        }
        // `--opt=value`
        let (name, inline) = match text.split_once('=') {
            Some((n, v)) if n.starts_with('-') && !v.is_empty() => (n, Some(v.to_string())),
            _ => (text, None),
        };
        let name_l = name.trim_start_matches('-').to_ascii_lowercase();
        let value = |i: &mut usize| -> Option<String> {
            if let Some(v) = &inline {
                return Some(v.clone());
            }
            let v = toks.get(*i).map(|t| t.text.clone());
            if v.is_some() {
                *i += 1;
            }
            v
        };
        match name_l.as_str() {
            "i" | "include" | "incdir" => s.includes.extend(value(&mut i)),
            "d" | "define" | "definemacro" => s.defines.extend(value(&mut i)),
            "l" => s.libs.extend(value(&mut i)),
            "work" => s.work = value(&mut i),
            "f" | "prj" | "filelist" => s.nested.extend(value(&mut i)),
            "generic_top" => s.generics.extend(value(&mut i)),
            "timescale" => s.timescale = value(&mut i),
            "top" => s.top.extend(value(&mut i)),
            "sv" | "systemverilog" => s.sv = true,
            "2008" => s.vhdl2008 = true,
            "relax" => s.relax = true,
            // Options that do not change what is simulated or only where logs go.
            "incr" | "nolog" | "nosort" | "sort" | "nostdout" | "stats" | "ignore_assertions"
            | "vlog95_compat" => {}
            "log" | "s" | "snapshot" | "mt" | "debug" | "verbose" | "v" | "o" | "o0" | "o1"
            | "o2" | "o3" | "pulse_e_style" | "pulse_r" | "pulse_e" | "rangecheck" | "cc_type"
            | "dll" | "wdb" | "initfile" | "lib_map_path" => {
                let _ = value(&mut i);
            }
            "y" | "sourcelibdir" | "sourcelibext" | "libext" | "uvm_version" => {
                let v = value(&mut i).unwrap_or_default();
                s.unknown.push((
                    format!("{text} {v}").trim().to_string(),
                    "library search directories are not supported; list the files explicitly"
                        .to_string(),
                ));
            }
            "sv_lib" | "sv_root" | "dpi_absolute" | "dpiheader" => {
                let v = value(&mut i).unwrap_or_default();
                s.unknown.push((
                    format!("{text} {v}").trim().to_string(),
                    "the export's own DPI libraries are not loaded; ZynqSim links its own \
                     `rp_*` library"
                        .to_string(),
                ));
            }
            _ => s.unknown.push((
                text.to_string(),
                "option is not understood and is ignored; results may differ from Vivado"
                    .to_string(),
            )),
        }
    }
    s
}

const PRJ_WORDS: &[&str] = &[
    "verilog",
    "sv",
    "systemverilog",
    "vhdl",
    "vhdl2008",
    "vhdl2019",
];

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
        && !s.chars().next().is_some_and(|c| c.is_ascii_digit())
}

impl Acc<'_> {
    fn diag(&mut self, sev: Severity, file: &Path, line: usize, construct: &str, reason: &str) {
        self.d.diagnostics.push(Diagnostic {
            severity: sev,
            file: file.to_path_buf(),
            line,
            construct: construct.to_string(),
            reason: reason.to_string(),
        });
    }

    fn lines(&mut self, path: &Path) -> Option<Vec<(usize, String)>> {
        match std::fs::read(path) {
            Ok(b) => Some(lex::logical_lines(&String::from_utf8_lossy(&b))),
            Err(e) => {
                self.diag(
                    Severity::Error,
                    path,
                    0,
                    &path.display().to_string(),
                    &format!("cannot be read: {e}"),
                );
                None
            }
        }
    }

    /// Tokenises one logical line, reporting unresolved variables.
    fn toks(&mut self, file: &Path, line: usize, text: &str) -> Vec<Tok> {
        let (t, unresolved) = self.toks_quiet(text);
        self.unresolved(file, line, unresolved);
        t
    }

    fn unresolved(&mut self, file: &Path, line: usize, names: Vec<String>) {
        for n in names {
            self.diag(
                Severity::Error,
                file,
                line,
                &format!("${n}"),
                "variable is not defined; set it in the environment (for Vivado: XILINX_VIVADO)",
            );
        }
    }

    fn toks_quiet(&mut self, text: &str) -> (Vec<Tok>, Vec<String>) {
        let mut unresolved = Vec::new();
        let vars = self.vars.clone();
        let r = self.r;
        let lookup = move |n: &str| {
            vars.iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.clone())
                .or_else(|| (r.env)(n))
        };
        let t = lex::tokenize(text, &lookup, &mut unresolved);
        (t, unresolved)
    }

    fn mark_seen(&mut self, p: &Path) -> bool {
        if self.seen.iter().any(|s| s == p) {
            return false;
        }
        self.seen.push(p.to_path_buf());
        self.d.inputs.push(p.to_path_buf());
        true
    }

    // ---- dispatch ------------------------------------------------------------------------

    fn any(&mut self, path: &Path, depth: usize) -> Result<(), String> {
        if path.is_dir() {
            self.expect_sources = true;
            return self.dir(path, depth);
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        if depth == 0 {
            self.expect_sources = !matches!(ext.as_str(), "opt" | "ini");
        }
        match ext.as_str() {
            "prj" => self.prj(path, depth),
            "opt" => self.opt(path, depth),
            "ini" => self.ini(path),
            "sh" | "bat" | "cmd" => self.script(path, depth),
            "v" | "sv" | "vhd" | "vhdl" => {
                // One netlist or source file: compiled into the default library with the
                // Xilinx simulation libraries searched (a funcsim netlist instantiates UNISIM
                // primitives).
                let lang = Lang::from_path(path).unwrap_or(Lang::Verilog);
                self.source(path, 0, Path::new(""), &path.display().to_string(), lang, "xil_defaultlib", &[], &[]);
                self.mark_seen(path);
                for l in ["unisims_ver", "unimacro_ver", "secureip"] {
                    self.link_lib(l, Path::new(""));
                }
            }
            _ if name == "xsim.ini" => self.ini(path),
            _ => {
                // A list in either style.
                let first = std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))
                    .and_then(|l| l.split_whitespace().next().map(str::to_ascii_lowercase))
                    .unwrap_or_default();
                if PRJ_WORDS.contains(&first.as_str()) {
                    self.prj(path, depth)
                } else {
                    self.filelist(path, depth)
                }
            }
        }
        Ok(())
    }

    fn dir(&mut self, dir: &Path, depth: usize) -> Result<(), String> {
        let found = walk::discover(dir);
        if found.is_empty() {
            return Err(format!(
                "{} holds no .prj, .f, elab.opt, xsim.ini or script (is it an export_simulation \
                 folder?)",
                dir.display()
            ));
        }
        if !found.ambiguous.is_empty() {
            let list: Vec<String> = found
                .ambiguous
                .iter()
                .map(|d| format!("  {}", d.display()))
                .collect();
            return Err(format!(
                "{} holds several exports; pass one of these folders:
{}",
                dir.display(),
                list.join("
")
            ));
        }
        // Scripts first: they name the lists in the order the vendor flow compiles them.
        // The batch script of a flow and its shell twin say the same; either is enough.
        let bat: Vec<_> = found
            .scripts
            .iter()
            .filter(|p| !p.extension().is_some_and(|e| e == "sh"))
            .cloned()
            .collect();
        let scripts = if bat.is_empty() { &found.scripts } else { &bat };
        for s in scripts {
            self.script(s, depth);
        }
        for p in &found.prjs {
            self.prj(p, depth);
        }
        if self.d.sources.is_empty() {
            for f in &found.lists {
                self.filelist(f, depth);
            }
        }
        for o in &found.opts {
            self.opt(o, depth);
        }
        for i in &found.inis {
            self.ini(i);
        }
        Ok(())
    }

    // ---- .prj ----------------------------------------------------------------------------

    fn prj(&mut self, path: &Path, depth: usize) {
        if depth > MAX_NESTING || !self.mark_seen(path) {
            return;
        }
        let Some(lines) = self.lines(path) else {
            return;
        };
        let base = path.parent().unwrap_or(Path::new("")).to_path_buf();
        for (no, text) in lines {
            let toks = self.toks(path, no, &text);
            let Some(first) = toks.first() else { continue };
            let word = first.text.to_ascii_lowercase();
            if matches!(word.as_str(), "nosort" | "sort") {
                continue;
            }
            let lang = match word.as_str() {
                "verilog" => Lang::Verilog,
                "sv" | "systemverilog" => Lang::SystemVerilog,
                "vhdl" => Lang::Vhdl,
                "vhdl2008" => Lang::Vhdl2008,
                "-f" | "-prj" | "--prj" | "--f" => {
                    if let Some(n) = toks.get(1) {
                        self.nested(path, no, &base, &n.text.clone(), depth);
                    }
                    continue;
                }
                _ => {
                    self.diag(
                        Severity::Warning,
                        path,
                        no,
                        &first.text,
                        "unknown project-file directive; the line is ignored",
                    );
                    continue;
                }
            };
            let sc = scan(&toks[1..]);
            // The first bare word is the library, when a file follows it.
            let mut pos = sc.positional.clone();
            let mut library = sc.work.clone();
            if pos.len() >= 2 && !pos[0].quoted && is_ident(&pos[0].text) {
                let l = pos.remove(0).text;
                library.get_or_insert(l);
            } else if pos.len() == 1 && !pos[0].quoted && is_ident(&pos[0].text) && library.is_none()
            {
                // `verilog work -f other.prj` style: a library with no file of its own.
                library = Some(pos.remove(0).text);
            }
            let library = library.unwrap_or_else(|| "work".to_string());
            let lang = match lang {
                Lang::Verilog if sc.sv => Lang::SystemVerilog,
                Lang::Vhdl if sc.vhdl2008 => Lang::Vhdl2008,
                l => l,
            };
            self.apply_scan(path, no, &base, &sc);
            let inc: Vec<PathBuf> = sc.includes.iter().map(|i| lex::resolve(&base, i)).collect();
            for f in pos {
                self.source(path, no, &base, &f.text, lang, &library, &inc, &sc.defines);
            }
            for n in &sc.nested {
                self.nested(path, no, &base, n, depth);
            }
        }
    }

    /// Records the options of a line that apply to the whole design.
    fn apply_scan(&mut self, file: &Path, line: usize, base: &Path, sc: &Scan) {
        for l in &sc.libs {
            self.link_lib(l, base);
        }
        if sc.relax {
            self.d.relax = true;
        }
        for (t, why) in &sc.unknown {
            self.diag(Severity::Warning, file, line, t, why);
        }
    }

    fn link_lib(&mut self, spec: &str, base: &Path) {
        let (name, dir) = match spec.split_once('=') {
            Some((n, d)) => (n, Some(d)),
            None => (spec, None),
        };
        if !name.is_empty() && !self.d.link_libs.iter().any(|l| l == name) {
            self.d.link_libs.push(name.to_string());
        }
        if let Some(d) = dir {
            let p = lex::resolve(base, d);
            if !self.d.lib_maps.iter().any(|(n, _)| n == name) {
                self.d.lib_maps.push((name.to_string(), p));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn source(
        &mut self,
        file: &Path,
        line: usize,
        base: &Path,
        path: &str,
        lang: Lang,
        library: &str,
        includes: &[PathBuf],
        defines: &[String],
    ) {
        self.d.sources.push(SourceFile {
            path: lex::resolve(base, path),
            lang,
            library: library.to_string(),
            includes: includes.to_vec(),
            defines: defines.to_vec(),
            origin: Origin {
                file: file.to_path_buf(),
                line,
            },
        });
    }

    fn nested(&mut self, file: &Path, line: usize, base: &Path, name: &str, depth: usize) {
        let p = lex::resolve(base, name);
        if depth + 1 > MAX_NESTING {
            self.diag(
                Severity::Error,
                file,
                line,
                name,
                "lists include each other too deeply",
            );
            return;
        }
        if !p.is_file() {
            self.diag(
                Severity::Error,
                file,
                line,
                name,
                "the included list does not exist",
            );
            return;
        }
        let _ = self.any(&p, depth + 1);
    }

    // ---- .f ------------------------------------------------------------------------------

    fn filelist(&mut self, path: &Path, depth: usize) {
        if depth > MAX_NESTING || !self.mark_seen(path) {
            return;
        }
        let Some(lines) = self.lines(path) else {
            return;
        };
        let base = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let mut work: Option<String> = None;
        let mut force_sv = false;
        for (no, text) in lines {
            let toks = self.toks(path, no, &text);
            if toks.is_empty() {
                continue;
            }
            let sc = scan(&toks);
            if let Some(w) = &sc.work {
                work = Some(w.clone());
            }
            force_sv |= sc.sv;
            for i in &sc.includes {
                let p = lex::resolve(&base, i);
                if !self.d.include_dirs.contains(&p) {
                    self.d.include_dirs.push(p);
                }
            }
            for d in &sc.defines {
                if !self.d.defines.contains(d) {
                    self.d.defines.push(d.clone());
                }
            }
            self.apply_scan(path, no, &base, &sc);
            for g in &sc.generics {
                self.d.generics.push(g.clone());
            }
            for n in &sc.nested {
                self.nested(path, no, &base, n, depth);
            }
            for t in sc.top {
                self.d.tops.push(t);
            }
            for f in &sc.positional {
                let p = Path::new(&f.text);
                let ext = p
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                match Lang::from_path(p) {
                    Some(l) => {
                        let l = if force_sv && l == Lang::Verilog {
                            Lang::SystemVerilog
                        } else {
                            l
                        };
                        let lib = work.clone().unwrap_or_else(|| "work".to_string());
                        self.source(path, no, &base, &f.text, l, &lib, &[], &[]);
                    }
                    // Headers are found through the include directories; data files are read
                    // by `$readmemh` relative to the run directory.
                    None if matches!(ext.as_str(), "vh" | "svh" | "h" | "mem" | "hex" | "dat") => {}
                    None => self.diag(
                        Severity::Warning,
                        path,
                        no,
                        &f.text,
                        "unknown file type (not .v/.sv/.vhd); it is not compiled",
                    ),
                }
            }
        }
    }

    // ---- elab.opt / xelab command ----------------------------------------------------------

    fn elab_tokens(&mut self, file: &Path, line: usize, toks: &[Tok]) {
        let base = file.parent().unwrap_or(Path::new("")).to_path_buf();
        let sc = scan(toks);
        for l in &sc.libs {
            self.link_lib(l, &base);
        }
        for n in &sc.nested {
            let p = lex::resolve(&base, n);
            if p.is_file() {
                let _ = self.any(&p, 1);
            }
        }
        for g in &sc.generics {
            self.d.generics.push(g.clone());
        }
        if sc.timescale.is_some() {
            self.d.timescale = sc.timescale.clone();
        }
        if sc.relax {
            self.d.relax = true;
        }
        for t in &sc.positional {
            if !self.d.tops.contains(&t.text) {
                self.d.tops.push(t.text.clone());
            }
        }
        for (t, why) in &sc.unknown {
            self.diag(Severity::Warning, file, line, t, why);
        }
    }

    fn opt(&mut self, path: &Path, depth: usize) {
        if depth > MAX_NESTING || !self.mark_seen(path) {
            return;
        }
        let Some(lines) = self.lines(path) else {
            return;
        };
        for (no, text) in lines {
            let toks = self.toks(path, no, &text);
            self.elab_tokens(path, no, &toks);
        }
    }

    // ---- xsim.ini ------------------------------------------------------------------------

    fn ini(&mut self, path: &Path) {
        if !self.mark_seen(path) {
            return;
        }
        let Some(lines) = self.lines(path) else {
            return;
        };
        let base = path.parent().unwrap_or(Path::new("")).to_path_buf();
        for (no, text) in lines {
            let t = text.trim();
            if t.is_empty() || t.starts_with("--") || t.starts_with('#') {
                continue;
            }
            let Some((name, dir)) = t.split_once('=') else {
                self.diag(
                    Severity::Warning,
                    path,
                    no,
                    t,
                    "not a `library=path` mapping; ignored",
                );
                continue;
            };
            let mut unresolved = Vec::new();
            let r = self.r;
            let dir = lex::expand(dir.trim(), &|n| (r.env)(n), &mut unresolved);
            if !unresolved.is_empty() {
                // A mapping into an install that is not here (for example a Vivado data
                // directory): the tool's own xsim.ini provides it.
                continue;
            }
            let name = name.trim().to_string();
            if !self.d.lib_maps.iter().any(|(n, _)| *n == name) {
                self.d.lib_maps.push((name, lex::resolve(&base, &dir)));
            }
        }
    }

    // ---- scripts -------------------------------------------------------------------------

    fn script(&mut self, path: &Path, depth: usize) {
        if depth > MAX_NESTING || !self.mark_seen(path) {
            return;
        }
        let Some(lines) = self.lines(path) else {
            return;
        };
        let base = path.parent().unwrap_or(Path::new("")).to_path_buf();
        self.vars.clear();
        for (no, raw) in lines {
            let line = raw.trim();
            let lower = line.to_ascii_lowercase();
            if line.is_empty()
                || line.starts_with('#')
                || lower.starts_with("rem ")
                || lower == "rem"
                || line.starts_with("::")
                || lower.starts_with("echo")
            {
                continue;
            }
            // `name="value"`, `export name=value`, `set name=value`
            let assign = lower
                .strip_prefix("set ")
                .or_else(|| lower.strip_prefix("export "))
                .map(|_| line.split_once(' ').map_or("", |x| x.1.trim()))
                .unwrap_or(line);
            if let Some((k, v)) = assign.split_once('=')
                && is_ident(k.trim())
                && !k.contains(' ')
            {
                let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
                self.vars.retain(|(n, _)| n != k.trim());
                self.vars.push((k.trim().to_string(), v));
                continue;
            }
            // Textual expansion of the script's own variables (they may hold several options).
            let mut text = line.to_string();
            let mut vars = self.vars.clone();
            vars.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
            for (k, v) in &vars {
                text = text
                    .replace(&format!("${{{k}}}"), v)
                    .replace(&format!("${k}"), v)
                    .replace(&format!("%{k}%"), v);
            }
            let (mut toks, unresolved) = self.toks_quiet(&text);
            // Wrappers in front of the tool name.
            while toks.first().is_some_and(|t| {
                matches!(
                    t.text.to_ascii_lowercase().as_str(),
                    "call" | "@call" | "execstep" | "time" | "exec" | "command"
                )
            }) {
                toks.remove(0);
            }
            let Some(cmd) = toks.first() else { continue };
            let name = cmd
                .text
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            let name = name
                .trim_end_matches(".bat")
                .trim_end_matches(".exe")
                .trim_end_matches(".sh");
            // Redirections and pipes end the command.
            let cut = toks
                .iter()
                .position(|t| {
                    !t.quoted
                        && (t.text.starts_with('>')
                            || t.text.starts_with("2>")
                            || t.text == "|"
                            || t.text == "&&"
                            || t.text == "||")
                })
                .unwrap_or(toks.len());
            let args = &toks[1..cut];
            match name {
                "xvlog" | "xvhdl" | "xelab" => {
                    // Variables of a shell line matter only when it runs one of the tools.
                    self.unresolved(path, no, unresolved);
                    if name == "xelab" {
                        self.elab_tokens(path, no, args);
                    } else {
                        self.compile_command(path, no, &base, name, args, depth);
                    }
                }
                _ => {}
            }
        }
        self.vars.clear();
    }

    fn compile_command(
        &mut self,
        file: &Path,
        line: usize,
        base: &Path,
        tool: &str,
        args: &[Tok],
        depth: usize,
    ) {
        let sc = scan(args);
        self.apply_scan(file, line, base, &sc);
        if sc.relax {
            self.d.relax = true;
        }
        for i in &sc.includes {
            let p = lex::resolve(base, i);
            if !self.d.include_dirs.contains(&p) {
                self.d.include_dirs.push(p);
            }
        }
        for d in &sc.defines {
            if !self.d.defines.contains(d) {
                self.d.defines.push(d.clone());
            }
        }
        for n in &sc.nested {
            self.nested(file, line, base, n, depth);
        }
        let default = if tool == "xvhdl" {
            Lang::Vhdl
        } else if sc.sv {
            Lang::SystemVerilog
        } else {
            Lang::Verilog
        };
        let lib = sc.work.clone().unwrap_or_else(|| "work".to_string());
        for f in &sc.positional {
            let l = Lang::from_path(Path::new(&f.text)).unwrap_or(default);
            self.source(file, line, base, &f.text, l, &lib, &[], &[]);
        }
    }

    // ---- finishing -------------------------------------------------------------------------

    fn finish(&mut self) {
        // Drop duplicates (a `.bat` and a `.sh` list the same files): first wins.
        let mut seen: Vec<(PathBuf, Lang, String)> = Vec::new();
        let mut sources = Vec::new();
        for s in std::mem::take(&mut self.d.sources) {
            let key = (s.path.clone(), s.lang, s.library.clone());
            if !seen.contains(&key) {
                seen.push(key);
                sources.push(s);
            }
        }
        // Missing files. `glbl.v` of an export normally comes from the Vivado install.
        let mut kept = Vec::new();
        for s in sources {
            if s.path.is_file() {
                kept.push(s);
                continue;
            }
            let is_glbl = s
                .path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("glbl.v"));
            if is_glbl {
                self.d.glbl_missing = true;
                continue;
            }
            let o = s.origin.clone();
            self.diag(
                Severity::Error,
                &o.file,
                o.line,
                &s.path.display().to_string(),
                "source file not found",
            );
        }
        self.d.sources = kept;
        let incs: Vec<PathBuf> = self
            .d
            .include_dirs
            .iter()
            .cloned()
            .chain(self.d.sources.iter().flat_map(|s| s.includes.clone()))
            .collect();
        let mut reported: Vec<PathBuf> = Vec::new();
        for i in incs {
            if !i.is_dir() && !reported.contains(&i) {
                self.diag(
                    Severity::Warning,
                    Path::new(""),
                    0,
                    &i.display().to_string(),
                    "include directory not found",
                );
                reported.push(i);
            }
        }
        // Nothing to compile is an error in itself.
        if self.expect_sources
            && self.d.sources.is_empty()
            && !self
                .d
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error)
        {
            self.diag(
                Severity::Error,
                Path::new(""),
                0,
                "sources",
                "no source files were found in the export",
            );
        }
    }
}
