//! Testbench generation for an AXI4-Lite slave IP.
//!
//! Reads the module header of a Verilog/SystemVerilog file, finds the AXI4-Lite
//! slave interface, the clock and the reset by their names, and writes the
//! `cosim_top.sv` that stands in for the PS7: it drives the IP from the
//! remote-port link exactly like the hand-written fixtures do. The header
//! parser is deliberately small (ANSI and non-ANSI port lists, parameters with
//! simple arithmetic, attributes and comments); it is not a Verilog parser.

use std::collections::HashMap;
use std::fmt::Write as _;

/// Direction of a port.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// `input`
    In,
    /// `output`
    Out,
    /// `inout`
    InOut,
}

/// One port of the module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    /// Name.
    pub name: String,
    /// Direction.
    pub dir: Dir,
    /// Width in bits.
    pub width: u32,
}

/// What was learnt from the module header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleInfo {
    /// Module name.
    pub name: String,
    /// Ports in declaration order.
    pub ports: Vec<Port>,
}

/// The AXI4-Lite slave interface found in a module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiLite {
    /// Common name prefix of the interface signals (`S_AXI_`).
    pub prefix: String,
    /// Port names by role (`awaddr`, `wdata`, ...), in the module's own case.
    pub signals: HashMap<&'static str, String>,
    /// Address width in bits.
    pub addr_width: u32,
    /// Clock port.
    pub clock: String,
    /// Reset port and whether it is active low.
    pub reset: Option<(String, bool)>,
}

const ROLES: [(&str, bool); 19] = [
    ("awaddr", true),
    ("awprot", false),
    ("awvalid", true),
    ("awready", true),
    ("wdata", true),
    ("wstrb", true),
    ("wvalid", true),
    ("wready", true),
    ("bresp", true),
    ("bvalid", true),
    ("bready", true),
    ("araddr", true),
    ("arprot", false),
    ("arvalid", true),
    ("arready", true),
    ("rdata", true),
    ("rresp", true),
    ("rvalid", true),
    ("rready", true),
];

// ----------------------------------------------------------------- lexing

/// Removes `//` and `/* */` comments and `(* *)` attributes.
fn strip(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        let two = |a: char, c: char| b[i] == a && i + 1 < b.len() && b[i + 1] == c;
        if two('/', '/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if two('/', '*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                i += 1;
            }
            i += 2;
        } else if two('(', '*') && !(i + 2 < b.len() && b[i + 2] == ')') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == ')') {
                i += 1;
            }
            i += 2;
            out.push(' ');
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// Text inside the balanced bracket pair starting at `s[open]`, and the index
/// after the closing bracket.
fn balanced(s: &str, open: usize) -> Option<(&str, usize)> {
    let b = s.as_bytes();
    let (o, c) = match b.get(open)? {
        b'(' => (b'(', b')'),
        b'[' => (b'[', b']'),
        b'{' => (b'{', b'}'),
        _ => return None,
    };
    let mut depth = 0i32;
    for (i, &ch) in b.iter().enumerate().skip(open) {
        if ch == o {
            depth += 1;
        } else if ch == c {
            depth -= 1;
            if depth == 0 {
                return Some((&s[open + 1..i], i + 1));
            }
        }
    }
    None
}

/// Splits at top-level commas (outside any bracket).
fn split_top(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in s.chars() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ => {}
        }
        if ch == sep && depth == 0 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(ch);
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

// ------------------------------------------------------------- expressions

struct Eval<'a> {
    t: Vec<char>,
    i: usize,
    vars: &'a HashMap<String, i64>,
}

impl Eval<'_> {
    fn ws(&mut self) {
        while self.i < self.t.len() && self.t[self.i].is_whitespace() {
            self.i += 1;
        }
    }

    fn expr(&mut self) -> Result<i64, String> {
        let mut v = self.term()?;
        loop {
            self.ws();
            match self.t.get(self.i) {
                Some('+') => {
                    self.i += 1;
                    v += self.term()?;
                }
                Some('-') => {
                    self.i += 1;
                    v -= self.term()?;
                }
                _ => return Ok(v),
            }
        }
    }

    fn term(&mut self) -> Result<i64, String> {
        let mut v = self.atom()?;
        loop {
            self.ws();
            match self.t.get(self.i) {
                Some('*') => {
                    self.i += 1;
                    v *= self.atom()?;
                }
                Some('/') => {
                    self.i += 1;
                    let d = self.atom()?;
                    if d == 0 {
                        return Err("division by zero".into());
                    }
                    v /= d;
                }
                _ => return Ok(v),
            }
        }
    }

    fn atom(&mut self) -> Result<i64, String> {
        self.ws();
        let c = *self.t.get(self.i).ok_or("unexpected end")?;
        if c == '(' {
            self.i += 1;
            let v = self.expr()?;
            self.ws();
            if self.t.get(self.i) == Some(&')') {
                self.i += 1;
            }
            return Ok(v);
        }
        if c == '-' {
            self.i += 1;
            return Ok(-self.atom()?);
        }
        if c.is_ascii_digit() || c == '\'' {
            let start = self.i;
            while self.i < self.t.len()
                && (self.t[self.i].is_ascii_alphanumeric()
                    || self.t[self.i] == '\''
                    || self.t[self.i] == '_')
            {
                self.i += 1;
            }
            let s: String = self.t[start..self.i]
                .iter()
                .filter(|c| **c != '_')
                .collect();
            return parse_number(&s);
        }
        if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let start = self.i;
            while self.i < self.t.len()
                && (self.t[self.i].is_ascii_alphanumeric()
                    || self.t[self.i] == '_'
                    || self.t[self.i] == '$')
            {
                self.i += 1;
            }
            let id: String = self.t[start..self.i].iter().collect();
            if id == "$clog2" {
                self.ws();
                if self.t.get(self.i) == Some(&'(') {
                    self.i += 1;
                    let v = self.expr()?;
                    self.ws();
                    if self.t.get(self.i) == Some(&')') {
                        self.i += 1;
                    }
                    let mut bits = 0;
                    while (1i64 << bits) < v.max(1) {
                        bits += 1;
                    }
                    return Ok(bits);
                }
            }
            return self
                .vars
                .get(&id)
                .copied()
                .ok_or_else(|| format!("unknown parameter `{id}`"));
        }
        Err(format!(
            "cannot evaluate `{}`",
            self.t.iter().collect::<String>()
        ))
    }
}

fn parse_number(s: &str) -> Result<i64, String> {
    // `32'd4`, `4'hF`, `'b1`, `12`.
    let (digits, radix) = match s.find('\'') {
        Some(p) => {
            let rest = &s[p + 1..];
            let rest = rest.trim_start_matches(['s', 'S']);
            let (r, d) = rest.split_at(1.min(rest.len()));
            let radix = match r {
                "d" | "D" => 10,
                "h" | "H" => 16,
                "b" | "B" => 2,
                "o" | "O" => 8,
                _ => return Err(format!("bad number `{s}`")),
            };
            (d.to_string(), radix)
        }
        None => (s.to_string(), 10),
    };
    i64::from_str_radix(&digits, radix).map_err(|_| format!("bad number `{s}`"))
}

fn eval(text: &str, vars: &HashMap<String, i64>) -> Result<i64, String> {
    let mut e = Eval {
        t: text.chars().collect(),
        i: 0,
        vars,
    };
    let v = e.expr()?;
    e.ws();
    if e.i != e.t.len() {
        return Err(format!("cannot evaluate `{text}`"));
    }
    Ok(v)
}

// ------------------------------------------------------------------ header

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Parameters (`parameter [integer] NAME = expr`) from a declaration list.
fn read_params(list: &str, vars: &mut HashMap<String, i64>) {
    for item in split_top(list, ',') {
        let mut t = item.trim();
        for kw in ["parameter", "localparam"] {
            if let Some(r) = t.strip_prefix(kw) {
                t = r.trim_start();
            }
        }
        let Some((lhs, rhs)) = t.split_once('=') else {
            continue;
        };
        // The name is the last identifier before `=` (after type words).
        let name: String = lhs
            .trim()
            .rsplit(|c: char| !is_ident_char(c))
            .next()
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        if let Ok(v) = eval(rhs.trim(), vars) {
            vars.insert(name, v);
        }
    }
}

fn parse_range(text: &str, vars: &HashMap<String, i64>) -> Result<u32, String> {
    let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
    let (msb, lsb) = inner
        .split_once(':')
        .ok_or_else(|| format!("unsupported range `{text}`"))?;
    let (m, l) = (eval(msb, vars)?, eval(lsb, vars)?);
    Ok((m - l).unsigned_abs() as u32 + 1)
}

/// Splits `input wire signed [3:0] name` style text into
/// `(dir, width_text, name)`; `dir` is `None` if no direction keyword.
fn port_item(item: &str) -> (Option<Dir>, Option<String>, String) {
    let t = item.split('=').next().unwrap_or("").trim();
    let mut rest = t;
    let mut dir = None;
    for (kw, d) in [
        ("input", Dir::In),
        ("output", Dir::Out),
        ("inout", Dir::InOut),
    ] {
        if let Some(r) = rest.strip_prefix(kw) {
            if r.starts_with(|c: char| c.is_whitespace() || c == '[') {
                dir = Some(d);
                rest = r.trim_start();
            }
        }
    }
    let mut range = None;
    loop {
        rest = rest.trim_start();
        let word_end = rest.find(|c: char| !is_ident_char(c)).unwrap_or(rest.len());
        let word = &rest[..word_end];
        if matches!(
            word,
            "wire" | "reg" | "logic" | "signed" | "unsigned" | "var" | "bit"
        ) {
            rest = &rest[word_end..];
        } else if rest.starts_with('[') {
            match balanced(rest, 0) {
                Some((_, end)) => {
                    range = Some(rest[..end].to_string());
                    rest = &rest[end..];
                }
                None => break,
            }
        } else {
            break;
        }
    }
    (dir, range, rest.trim().to_string())
}

/// Parses the header of module `want` (or the first module).
pub fn parse_module(src: &str, want: Option<&str>) -> Result<ModuleInfo, String> {
    let s = strip(src);
    let mut search = 0;
    let (name, after) = loop {
        let Some(p) = s[search..].find("module") else {
            return Err(match want {
                Some(w) => format!("module `{w}` was not found"),
                None => "no module found".to_string(),
            });
        };
        let at = search + p;
        let before_ok = at == 0 || !is_ident_char(s[..at].chars().last().unwrap_or(' '));
        let tail = &s[at + 6..];
        search = at + 6;
        if !before_ok || !tail.starts_with(|c: char| c.is_whitespace()) {
            continue;
        }
        let tail_trim = tail.trim_start();
        let n_end = tail_trim
            .find(|c: char| !is_ident_char(c))
            .unwrap_or(tail_trim.len());
        let name = &tail_trim[..n_end];
        if name.is_empty() || name == "endmodule" {
            continue;
        }
        if want.is_none_or(|w| w == name) {
            let off = s.len() - tail_trim.len() + n_end;
            break (name.to_string(), off);
        }
    };

    let mut vars: HashMap<String, i64> = HashMap::new();
    let mut i = after;
    let bytes = s.as_bytes();
    let skip_ws = |i: &mut usize| {
        while *i < bytes.len() && (bytes[*i] as char).is_whitespace() {
            *i += 1;
        }
    };
    skip_ws(&mut i);
    if bytes.get(i) == Some(&b'#') {
        i += 1;
        skip_ws(&mut i);
        let (list, end) = balanced(&s, i).ok_or("unbalanced parameter list")?;
        read_params(list, &mut vars);
        i = end;
        skip_ws(&mut i);
    }
    let (plist, end) = if bytes.get(i) == Some(&b'(') {
        balanced(&s, i).ok_or("unbalanced port list")?
    } else {
        ("", i)
    };
    let body_start = s[end..].find(';').map_or(end, |p| end + p + 1);
    let body_end = s[body_start..]
        .find("endmodule")
        .map_or(s.len(), |p| body_start + p);
    let body = &s[body_start..body_end];

    // Body parameters and localparams, one statement each.
    for stmt in body.split(';') {
        let t = stmt.trim();
        if t.starts_with("parameter") || t.starts_with("localparam") {
            read_params(t, &mut vars);
        }
    }

    let items = split_top(plist, ',');
    let mut ports = Vec::new();
    let ansi = items.first().is_some_and(|it| port_item(it).0.is_some());
    if ansi {
        let mut cur_dir = Dir::In;
        let mut cur_range: Option<String> = None;
        for it in &items {
            let (d, r, n) = port_item(it);
            if let Some(d) = d {
                cur_dir = d;
                cur_range = r;
            } else if r.is_some() {
                cur_range = r;
            }
            if n.is_empty() {
                continue;
            }
            let width = match &cur_range {
                Some(r) => parse_range(r, &vars).map_err(|e| format!("port `{n}`: {e}"))?,
                None => 1,
            };
            ports.push(Port {
                name: n,
                dir: cur_dir,
                width,
            });
        }
    } else {
        // Non-ANSI: names in the header, directions in the body.
        let names: Vec<String> = items.iter().map(|s| s.trim().to_string()).collect();
        let mut found: HashMap<String, Port> = HashMap::new();
        for stmt in body.split(';') {
            let t = stmt.trim();
            if !(t.starts_with("input") || t.starts_with("output") || t.starts_with("inout")) {
                continue;
            }
            let (d, r, _) = port_item(t.split(',').next().unwrap_or(t));
            let Some(d) = d else { continue };
            let width = match &r {
                Some(r) => parse_range(r, &vars)?,
                None => 1,
            };
            // Every declared name in this statement.
            let first = t.split(',').next().unwrap_or("");
            let (_, _, n0) = port_item(first);
            let mut all = vec![n0];
            all.extend(t.split(',').skip(1).map(|x| x.trim().to_string()));
            for n in all {
                found.insert(
                    n.clone(),
                    Port {
                        name: n,
                        dir: d,
                        width,
                    },
                );
            }
        }
        for n in names {
            match found.remove(&n) {
                Some(p) => ports.push(p),
                None => return Err(format!("port `{n}` has no input/output declaration")),
            }
        }
    }
    if ports.is_empty() {
        return Err(format!("module `{name}` has no ports"));
    }
    Ok(ModuleInfo { name, ports })
}

// -------------------------------------------------------------- detection

/// Finds the AXI4-Lite slave interface, clock and reset of `m`.
pub fn detect_axi_lite(m: &ModuleInfo) -> Result<AxiLite, String> {
    // Group ports by the prefix that precedes a role suffix.
    let mut groups: HashMap<String, HashMap<&'static str, &Port>> = HashMap::new();
    for p in &m.ports {
        let lower = p.name.to_ascii_lowercase();
        for (role, _) in ROLES {
            if let Some(prefix) = lower.strip_suffix(role) {
                groups
                    .entry(prefix.to_string())
                    .or_default()
                    .insert(role, p);
            }
        }
    }
    let (prefix_lc, roles) = groups
        .into_iter()
        // Slave interfaces take addresses in; a group whose address is an
        // output is a master (see `detect_axi_master`).
        .filter(|(_, r)| r.len() >= 8 && r.get("awaddr").is_none_or(|p| p.dir == Dir::In))
        .max_by_key(|(prefix, r)| (r.len(), prefix.len()))
        .ok_or_else(|| {
            format!(
                "`{}` has no AXI4-Lite slave interface: expected ports like S_AXI_AWADDR, \
                 S_AXI_WDATA, S_AXI_ARVALID ... (a master, AXI4 full or AXI-Stream IP needs a \
                 hand-written testbench, see fixtures/rtl/dma)",
                m.name
            )
        })?;
    let mut missing = Vec::new();
    for (role, required) in ROLES {
        if required && !roles.contains_key(role) {
            missing.push(role);
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "the AXI4-Lite interface `{prefix_lc}*` of `{}` lacks: {}",
            m.name,
            missing.join(", ")
        ));
    }
    let dw = roles["wdata"].width;
    if dw != 32 {
        return Err(format!(
            "the data width is {dw}: the Zynq GP ports are 32 bits wide, so only 32 is supported"
        ));
    }
    let aw = roles["awaddr"].width;
    let mut signals = HashMap::new();
    for (role, p) in &roles {
        signals.insert(*role, p.name.clone());
    }

    // Clock and reset among the input ports.
    let inputs: Vec<&Port> = m.ports.iter().filter(|p| p.dir == Dir::In).collect();
    let lc = |p: &Port| p.name.to_ascii_lowercase();
    let clock = inputs
        .iter()
        .find(|p| lc(p) == format!("{prefix_lc}aclk"))
        .or_else(|| inputs.iter().find(|p| lc(p) == format!("{prefix_lc}clk")))
        .or_else(|| inputs.iter().find(|p| lc(p).contains("aclk")))
        .or_else(|| {
            inputs
                .iter()
                .find(|p| matches!(lc(p).as_str(), "clk" | "clock" | "aclk"))
        })
        .or_else(|| inputs.iter().find(|p| lc(p).contains("clk")))
        .map(|p| p.name.clone())
        .ok_or_else(|| {
            format!(
                "`{}` has no clock input (a port named *aclk or *clk)",
                m.name
            )
        })?;
    let low = |n: &str| {
        let n = n.to_ascii_lowercase();
        n.contains("aresetn")
            || n.contains("resetn")
            || n.contains("rstn")
            || n.contains("reset_n")
            || n.contains("rst_n")
            || n.ends_with("_n")
    };
    let reset = inputs
        .iter()
        .find(|p| lc(p) == format!("{prefix_lc}aresetn"))
        .or_else(|| inputs.iter().find(|p| low(&p.name)))
        .or_else(|| {
            inputs
                .iter()
                .find(|p| lc(p).contains("reset") || lc(p).contains("rst"))
        })
        .map(|p| (p.name.clone(), low(&p.name)));
    // The prefix in the module's own case.
    let prefix = roles["awaddr"].name[..prefix_lc.len()].to_string();
    Ok(AxiLite {
        prefix,
        signals,
        addr_width: aw,
        clock,
        reset,
    })
}

// ------------------------------------------------------------- generation

fn decl(width: u32) -> String {
    if width > 1 {
        format!("[{}:0] ", width - 1)
    } else {
        String::new()
    }
}

const MASTER_ROLES: [&str; 25] = [
    "araddr", "arlen", "arsize", "arburst", "arvalid", "arready", "rdata", "rresp", "rlast",
    "rvalid", "rready", "awaddr", "awlen", "awsize", "awburst", "awvalid", "awready", "wdata",
    "wstrb", "wlast", "wvalid", "wready", "bresp", "bvalid", "bready",
];

/// An AXI4 master interface (the IP reads and writes memory through it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiMaster {
    /// Common name prefix of the interface signals (`m_`).
    pub prefix: String,
    /// Port names by role, in the module's own case.
    pub signals: HashMap<&'static str, String>,
}

/// Finds an AXI4 master interface, if the module has one. `Err` when there is
/// one but it cannot be driven (missing signals, not 32 bits).
pub fn detect_axi_master(m: &ModuleInfo) -> Result<Option<AxiMaster>, String> {
    let mut groups: HashMap<String, HashMap<&'static str, &Port>> = HashMap::new();
    for p in &m.ports {
        let lower = p.name.to_ascii_lowercase();
        for role in MASTER_ROLES {
            if let Some(prefix) = lower.strip_suffix(role) {
                groups
                    .entry(prefix.to_string())
                    .or_default()
                    .insert(role, p);
            }
        }
    }
    let Some((prefix_lc, roles)) = groups
        .into_iter()
        .filter(|(_, r)| r.get("awaddr").is_some_and(|p| p.dir == Dir::Out) && r.len() >= 8)
        .max_by_key(|(prefix, r)| (r.len(), prefix.len()))
    else {
        return Ok(None);
    };
    let missing: Vec<&str> = MASTER_ROLES
        .iter()
        .copied()
        .filter(|r| !roles.contains_key(r))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "the AXI4 master interface `{prefix_lc}*` of `{}` lacks: {}",
            m.name,
            missing.join(", ")
        ));
    }
    for r in ["rdata", "wdata"] {
        if roles[r].width != 32 {
            return Err(format!(
                "the AXI4 master data width is {}: only 32 bits are supported",
                roles[r].width
            ));
        }
    }
    let signals = roles.iter().map(|(r, p)| (*r, p.name.clone())).collect();
    let prefix = roles["awaddr"].name[..prefix_lc.len()].to_string();
    Ok(Some(AxiMaster { prefix, signals }))
}

/// File name of the protocol checker module, written next to the generated testbench.
pub const PROTOCOL_CHECKER_FILE: &str = "zs_axi_checker.sv";

/// The source of the AXI protocol checker (`zs_axi_checker`, ADR-025). `init --ip` writes it
/// next to the testbench and lists it in `project.toml`.
pub fn protocol_checker_source() -> &'static str {
    include_str!("../../../fixtures/rtl/common/zs_axi_checker.sv")
}

/// Width of port `name` of `m` in bits, if it has such a port.
fn port_width(m: &ModuleInfo, name: &str) -> Option<u32> {
    m.ports.iter().find(|p| p.name == name).map(|p| p.width)
}

/// The protocol checkers of the testbench: one on the AXI4-Lite slave the testbench drives,
/// one on the AXI4 master the IP drives (when it has one). They observe only. They are
/// compiled out with the define `ZS_NO_PROTOCOL_CHECKERS` (`[rtl] protocol_checkers = false`).
fn protocol_checkers(m: &ModuleInfo, axi: &AxiLite, master: Option<&AxiMaster>) -> String {
    let g = |r: &str| axi.signals.get(r).cloned().unwrap_or_default();
    let dw = axi
        .signals
        .get("wdata")
        .and_then(|n| port_width(m, n))
        .unwrap_or(32);
    let mut s = format!(
        "`ifndef ZS_NO_PROTOCOL_CHECKERS
    // AXI protocol checkers (ADR-025): they observe the bus and drive nothing.
    zs_axi_checker #(.ID(0), .FULL(0), .AW({aw}), .DW({dw})) zs_chk_lite (
        .aclk(aclk), .aresetn(aresetn),
        .awvalid({awvalid}), .awready({awready}), .awaddr({awaddr}),
        .awlen(8'd0), .awsize(3'd2), .awburst(2'd1),
        .wvalid({wvalid}), .wready({wready}), .wdata({wdata}), .wstrb({wstrb}), .wlast(1'b1),
        .bvalid({bvalid}), .bready({bready}), .bresp({bresp}),
        .arvalid({arvalid}), .arready({arready}), .araddr({araddr}),
        .arlen(8'd0), .arsize(3'd2), .arburst(2'd1),
        .rvalid({rvalid}), .rready({rready}), .rdata({rdata}), .rresp({rresp}), .rlast(1'b1)
    );
",
        aw = axi.addr_width,
        awvalid = g("awvalid"),
        awready = g("awready"),
        awaddr = g("awaddr"),
        wvalid = g("wvalid"),
        wready = g("wready"),
        wdata = g("wdata"),
        wstrb = g("wstrb"),
        bvalid = g("bvalid"),
        bready = g("bready"),
        bresp = g("bresp"),
        arvalid = g("arvalid"),
        arready = g("arready"),
        araddr = g("araddr"),
        rvalid = g("rvalid"),
        rready = g("rready"),
        rdata = g("rdata"),
        rresp = g("rresp"),
    );
    if let Some(mst) = master {
        let n = |r: &str| mst.signals[r].clone();
        let maw = port_width(m, &n("awaddr")).unwrap_or(32);
        let mdw = port_width(m, &n("wdata")).unwrap_or(32);
        let _ = write!(
            s,
            "
    zs_axi_checker #(.ID(1), .FULL(1), .AW({maw}), .DW({mdw})) zs_chk_mst (
        .aclk(aclk), .aresetn(aresetn),
        .awvalid({awvalid}), .awready({awready}), .awaddr({awaddr}),
        .awlen({awlen}), .awsize({awsize}), .awburst({awburst}),
        .wvalid({wvalid}), .wready({wready}), .wdata({wdata}), .wstrb({wstrb}), .wlast({wlast}),
        .bvalid({bvalid}), .bready({bready}), .bresp({bresp}),
        .arvalid({arvalid}), .arready({arready}), .araddr({araddr}),
        .arlen({arlen}), .arsize({arsize}), .arburst({arburst}),
        .rvalid({rvalid}), .rready({rready}), .rdata({rdata}), .rresp({rresp}), .rlast({rlast})
    );
",
            awvalid = n("awvalid"),
            awready = n("awready"),
            awaddr = n("awaddr"),
            awlen = n("awlen"),
            awsize = n("awsize"),
            awburst = n("awburst"),
            wvalid = n("wvalid"),
            wready = n("wready"),
            wdata = n("wdata"),
            wstrb = n("wstrb"),
            wlast = n("wlast"),
            bvalid = n("bvalid"),
            bready = n("bready"),
            bresp = n("bresp"),
            arvalid = n("arvalid"),
            arready = n("arready"),
            araddr = n("araddr"),
            arlen = n("arlen"),
            arsize = n("arsize"),
            arburst = n("arburst"),
            rvalid = n("rvalid"),
            rready = n("rready"),
            rdata = n("rdata"),
            rresp = n("rresp"),
            rlast = n("rlast"),
        );
    }
    s.push_str(
        "`endif
",
    );
    s
}

/// Remote-port device number for an AXI master interface name (e.g. `S_AXI_HP0` -> 2, `S_AXI_ACP` -> 6).
pub fn master_interface_device(iface: &str) -> Option<u32> {
    match iface {
        "S_AXI_GP0" => Some(0),
        "S_AXI_GP1" => Some(1),
        "S_AXI_HP0" => Some(2),
        "S_AXI_HP1" => Some(3),
        "S_AXI_HP2" => Some(4),
        "S_AXI_HP3" => Some(5),
        "S_AXI_ACP" => Some(6),
        _ => None,
    }
}

/// The memory bridge: AXI4 bursts of the IP's master become remote-port
/// accesses to the PS (e.g. S_AXI_HP0 or S_AXI_ACP), with a modelled DDR latency.
pub fn master_bridge_with_dev(mst: &AxiMaster, dev: u32, pfx: &str) -> String {
    let g = |r: &str| {
        let sig = mst.signals[r].as_str();
        if pfx.is_empty() {
            sig.to_string()
        } else {
            format!("{pfx}_{sig}")
        }
    };
    let dev_name = match dev {
        0 => "S_AXI_GP0",
        1 => "S_AXI_GP1",
        2 => "S_AXI_HP0",
        3 => "S_AXI_HP1",
        4 => "S_AXI_HP2",
        5 => "S_AXI_HP3",
        6 => "S_AXI_ACP",
        _ => "PS memory",
    };
    let var_pfx = if pfx.is_empty() {
        String::new()
    } else {
        format!("{pfx}_")
    };
    let dev_const = if pfx.is_empty() {
        "HP0_DEV".to_string()
    } else {
        format!("{pfx}_DEV")
    };
    let lat_const = if pfx.is_empty() {
        "DDR_LAT".to_string()
    } else {
        format!("{pfx}_DDR_LAT")
    };

    format!(
        "
    // ---- {prefix}* (AXI4 master) -> PS memory through {dev_name} ----
    localparam int {dev_const} = {dev};     // remote-port device of {dev_name}
    localparam int {lat_const} = 16;    // modelled DDR latency seen by the IP, in clocks

    reg [31:0] {var_pfx}ra; reg [7:0] {var_pfx}rl; integer {var_pfx}rslot, {var_pfx}rst, {var_pfx}ri;
    initial begin
        wait (connected);
        forever begin
            @(posedge aclk);
            if ({arvalid} && !{arready}) begin
                {var_pfx}ra = {araddr}; {var_pfx}rl = {arlen};
                {arready} <= 1'b1;
                @(posedge aclk);
                {arready} <= 1'b0;
                {var_pfx}rslot = rp_mem_start($time, {dev_const}, 0, {var_pfx}ra, ({var_pfx}rl + 1) * 4);
                {var_pfx}rst = rp_mem_wait({var_pfx}rslot);
                repeat ({lat_const}) @(posedge aclk);
                for ({var_pfx}ri = 0; {var_pfx}ri <= {var_pfx}rl; {var_pfx}ri = {var_pfx}ri + 1) begin
                    {rdata}  <= ({var_pfx}rst == 1) ? rp_mem_get({var_pfx}rslot, {var_pfx}ri) : 32'hBAD0_BAD0;
                    {rresp}  <= ({var_pfx}rst == 1) ? 2'b00 : 2'b10;
                    {rvalid} <= 1'b1;
                    {rlast}  <= ({var_pfx}ri == {var_pfx}rl);
                    @(posedge aclk);
                    while (!{rready}) @(posedge aclk);
                    {rvalid} <= 1'b0; {rlast} <= 1'b0;
                end
                rp_mem_free({var_pfx}rslot);
            end
        end
    end

    reg [31:0] {var_pfx}wa; reg [7:0] {var_pfx}wl; integer {var_pfx}wslot, {var_pfx}wst, {var_pfx}wi;
    initial begin
        wait (connected);
        forever begin
            @(posedge aclk);
            if ({awvalid} && !{awready}) begin
                {var_pfx}wa = {awaddr}; {var_pfx}wl = {awlen};
                {awready} <= 1'b1;
                @(posedge aclk);
                {awready} <= 1'b0;
                for ({var_pfx}wi = 0; {var_pfx}wi <= {var_pfx}wl; {var_pfx}wi = {var_pfx}wi + 1) begin
                    {wready} <= 1'b1;
                    @(posedge aclk);
                    while (!{wvalid}) @(posedge aclk);
                    rp_mem_put({var_pfx}wi, {wdata});
                    {wready} <= 1'b0;
                end
                {var_pfx}wslot = rp_mem_start($time, {dev_const}, 1, {var_pfx}wa, ({var_pfx}wl + 1) * 4);
                {var_pfx}wst = rp_mem_wait({var_pfx}wslot);
                repeat ({lat_const}) @(posedge aclk);
                rp_mem_free({var_pfx}wslot);
                {bvalid} <= 1'b1; {bresp} <= ({var_pfx}wst == 1) ? 2'b00 : 2'b10;
                @(posedge aclk);
                while (!{bready}) @(posedge aclk);
                {bvalid} <= 1'b0;
            end
        end
    end
",
        prefix = mst.prefix,
        dev_const = dev_const,
        lat_const = lat_const,
        dev = dev,
        dev_name = dev_name,
        var_pfx = var_pfx,
        arvalid = g("arvalid"),
        arready = g("arready"),
        araddr = g("araddr"),
        arlen = g("arlen"),
        rdata = g("rdata"),
        rresp = g("rresp"),
        rvalid = g("rvalid"),
        rlast = g("rlast"),
        rready = g("rready"),
        awvalid = g("awvalid"),
        awready = g("awready"),
        awaddr = g("awaddr"),
        awlen = g("awlen"),
        wready = g("wready"),
        wvalid = g("wvalid"),
        wdata = g("wdata"),
        bvalid = g("bvalid"),
        bresp = g("bresp"),
        bready = g("bready"),
    )
}

/// The memory bridge: AXI4 bursts of the IP's master become remote-port
/// accesses to the PS (S_AXI_HP0), with a modelled DDR latency.
fn master_bridge(mst: &AxiMaster) -> String {
    master_bridge_with_dev(mst, 2, "")
}

/// A port of the IP that is a Pmod port or the RGB LEDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinGroup {
    /// Port name.
    pub name: String,
    /// Group: 0..=4 are JA..JE, 5 is the camera connector, 6 the RGB LEDs.
    pub group: u8,
    /// True when the IP drives it (an output), false for a host-driven input.
    pub output: bool,
    /// True when the module has both an output and an input port on this
    /// group (a bidirectional bus: I2C, SPI with MISO). The testbench then
    /// asks the board's devices for the input after every output change.
    pub paired: bool,
}

/// Eight-bit ports named like a Pmod (`pmod_ja`, `ja`, `pmod_jb_o`...) and
/// six-bit ports named like the RGB LEDs (`rgb`, `rgb_leds`, `ld5_6`).
pub fn detect_pin_groups(m: &ModuleInfo) -> Vec<PinGroup> {
    let mut out = Vec::new();
    for p in &m.ports {
        if p.dir == Dir::InOut {
            continue;
        }
        let mut l = p.name.to_ascii_lowercase();
        for suf in ["_out", "_in", "_o", "_i", "_pins"] {
            if let Some(r) = l.strip_suffix(suf) {
                l = r.to_string();
                break;
            }
        }
        let l = l.replace("pmod_", "").replace("pmod", "");
        let group = match (l.as_str(), p.width) {
            ("ja", 8) => Some(0),
            ("jb", 8) => Some(1),
            ("jc", 8) => Some(2),
            ("jd", 8) => Some(3),
            ("je", 8) => Some(4),
            ("cam" | "pcam", 8) => Some(5),
            ("rgb" | "rgb_led" | "rgb_leds" | "ld5_6", 6) => Some(6),
            _ => None,
        };
        if let Some(group) = group {
            out.push(PinGroup {
                name: p.name.clone(),
                group,
                output: p.dir == Dir::Out,
                paired: false,
            });
        }
    }
    // Pmod ports only: the RGB LEDs are outputs of the board.
    let pairs: Vec<u8> = out
        .iter()
        .filter(|g| g.group < 6 && g.output)
        .map(|g| g.group)
        .filter(|gr| out.iter().any(|i| !i.output && i.group == *gr))
        .collect();
    for g in &mut out {
        g.paired = pairs.contains(&g.group);
    }
    out
}

/// One-bit outputs that look like interrupt lines (`irq`, `interrupt`,
/// `intr` in the name), in declaration order. They become `IRQ_F2P[0..]`.
pub fn detect_irqs(m: &ModuleInfo, axi: &AxiLite) -> Vec<String> {
    let axi_names: Vec<&String> = axi.signals.values().collect();
    m.ports
        .iter()
        .filter(|p| p.dir == Dir::Out && p.width == 1 && !axi_names.contains(&&p.name))
        .filter(|p| {
            let l = p.name.to_ascii_lowercase();
            l.contains("irq") || l.contains("interrupt") || l.contains("intr")
        })
        .map(|p| p.name.clone())
        .take(16)
        .collect()
}

/// The testbench text for `m` and its interface `axi`.
pub fn generate_testbench(m: &ModuleInfo, axi: &AxiLite) -> String {
    generate_testbench_with_irqs(m, axi, None)
}

/// The `rp_*` DPI imports of the testbench, with their Icarus Verilog form (ADR-037).
///
/// Without `ZS_ICARUS` the text is the `import "DPI-C"` lines the DPI simulators (XSim,
/// Verilator, Questa) take. With `ZS_ICARUS` (Icarus has no DPI-C) each import is a wrapper of
/// the system task or function `$zs_<name>` that `zynqsim.vpi` registers. `rp_next` has output
/// arguments, which an Icarus function cannot take, so it has no wrapper: its call site is
/// wrapped instead (see [`RP_NEXT_CALL`]).
const RP_IMPORTS: &str = r#"`ifdef ZS_ICARUS
    // Icarus Verilog has no DPI-C (ADR-037): each import is a wrapper of the VPI system call
    // `$zs_<name>` of zynqsim.vpi. A `$time` passed straight to a system call reads as 0 in
    // vvp, so the time of `$zs_rp_next` goes through `zs_now` (see RP_NEXT_CALL).
    longint zs_now;
    function int rp_open(input string descr);
        rp_open = $zs_rp_open(descr);
    endfunction
    task rp_done_t(input int rdata, input int ok, input longint now_ns);
        $zs_rp_done_t(rdata, ok, now_ns);
    endtask
    task rp_close();
        $zs_rp_close();
    endtask
    task rp_lockstep(input int on);
        $zs_rp_lockstep(on);
    endtask
    function longint rp_sync_target();
        rp_sync_target = $zs_rp_sync_target();
    endfunction
    task rp_sync_ack(input longint now_ns);
        $zs_rp_sync_ack(now_ns);
    endtask
    function longint rp_req_ts();
        rp_req_ts = $zs_rp_req_ts();
    endfunction
    task rp_irq(input longint now_ns, input int line, input int val);
        $zs_rp_irq(now_ns, line, val);
    endtask
    task rp_mem_put(input int idx, input int word);
        $zs_rp_mem_put(idx, word);
    endtask
    function int rp_mem_start(input longint now_ns, input int dev,
                                        input int is_write, input int addr, input int len);
        rp_mem_start = $zs_rp_mem_start(now_ns, dev, is_write, addr, len);
    endfunction
    function int rp_mem_wait(input int slot);
        rp_mem_wait = $zs_rp_mem_wait(slot);
    endfunction
    function int rp_mem_get(input int slot, input int idx);
        rp_mem_get = $zs_rp_mem_get(slot, idx);
    endfunction
    task rp_mem_free(input int slot);
        $zs_rp_mem_free(slot);
    endtask
`else
    import "DPI-C" function int  rp_open(input string descr);
    import "DPI-C" function int  rp_next(input longint now_ns, input int idle_ms,
                                         output int addr, output int wdata,
                                         output int wstrb, output int is_write);
    import "DPI-C" function void rp_done_t(input int rdata, input int ok, input longint now_ns);
    import "DPI-C" function void rp_close();
    import "DPI-C" function void rp_lockstep(input int on);
    import "DPI-C" function longint rp_sync_target();
    import "DPI-C" function void rp_sync_ack(input longint now_ns);
    import "DPI-C" function longint rp_req_ts();
    import "DPI-C" function void rp_irq(input longint now_ns, input int line, input int val);
    import "DPI-C" function void rp_mem_put(input int idx, input int word);
    import "DPI-C" function int  rp_mem_start(input longint now_ns, input int dev,
                                              input int is_write, input int addr, input int len);
    import "DPI-C" function int  rp_mem_wait(input int slot);
    import "DPI-C" function int  rp_mem_get(input int slot, input int idx);
    import "DPI-C" function void rp_mem_free(input int slot);
`endif
"#;

/// The call of `rp_next` in the testbench's loop, with its Icarus form: the system function
/// writes its outputs itself (Icarus functions cannot have output arguments).
const RP_NEXT_CALL: &str = "`ifdef ZS_ICARUS
            zs_now = $time;
            st = $zs_rp_next(zs_now, -1, a_i, wd_i, ws_i, iw_i);
`else
            st = rp_next($time, -1, a_i, wd_i, ws_i, iw_i);
`endif
";

/// The pin-group DPI imports of the testbench, with their Icarus wrappers (ADR-037).
const PIN_IMPORTS: &str = r#"`ifdef ZS_ICARUS
    task zs_pin_out(input int group, input int levels, input longint now_ns);
        $zs_pin_out(group, levels, now_ns);
    endtask
    function int zs_pin_in(input int group);
        zs_pin_in = $zs_pin_in(group);
    endfunction
    function int zs_pin_xchg(input int group, input int levels, input longint now_ns);
        zs_pin_xchg = $zs_pin_xchg(group, levels, now_ns);
    endfunction
`else
    import "DPI-C" function void zs_pin_out(input int group, input int levels, input longint now_ns);
    import "DPI-C" function int  zs_pin_in(input int group);
    import "DPI-C" function int  zs_pin_xchg(input int group, input int levels, input longint now_ns);
`endif
"#;

/// Like [`generate_testbench`], with the `IRQ_F2P` bit of each interrupt output.
///
/// `irq_bits[k]` is the bit of the k-th output [`detect_irqs`] reports (`None` leaves that
/// output unconnected, as the block design did). `None` for the whole slice numbers them
/// `0..`, which is what `init --ip` has always done.
pub fn generate_testbench_with_irqs(
    m: &ModuleInfo,
    axi: &AxiLite,
    irq_bits: Option<&[Option<u32>]>,
) -> String {
    let irqs = detect_irqs(m, axi);
    let pin_groups = detect_pin_groups(m);
    let master = detect_axi_master(m).ok().flatten();
    let sig = |r: &str| axi.signals.get(r).cloned();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "`timescale 1ns / 1ps\n\
         // Generated by `zynqsim init --ip`: stands in for the PS7 and drives `{}`\n\
         // through its AXI4-Lite slave `{}*` from the remote-port link (lock-step).\n\
         // Edit freely: it is a plain testbench. A write of 0xC051D0DE to offset 0x3C ends the run.\n\
         module cosim_top;",
        m.name, axi.prefix
    );
    s.push_str(RP_IMPORTS);
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "    reg aclk = 1'b0;\n    reg aresetn = 1'b0;\n    always #5 aclk = ~aclk;   // 100 MHz = FCLK_CLK0\n"
    );

    // Signal declarations for every port.
    let mut role_of: HashMap<&str, &str> =
        axi.signals.iter().map(|(r, n)| (n.as_str(), *r)).collect();
    if let Some(mst) = &master {
        for (r, n) in &mst.signals {
            role_of.insert(n.as_str(), *r);
        }
    }
    for p in &m.ports {
        let n = &p.name;
        if *n == axi.clock || axi.reset.as_ref().is_some_and(|(r, _)| r == n) {
            continue;
        }
        let w = decl(p.width);
        match (p.dir, role_of.get(n.as_str())) {
            (Dir::In, Some(_)) => {
                let _ = writeln!(s, "    reg  {w}{n} = 0;");
            }
            (Dir::In, None) if pin_groups.iter().any(|g| &g.name == n && g.paired) => {
                let _ = writeln!(
                    s,
                    "    reg  {w}{n} = '1;     // driven by the board's devices (released = 1)"
                );
            }
            (Dir::In, None) if pin_groups.iter().any(|g| &g.name == n) => {
                let _ = writeln!(
                    s,
                    "    reg  {w}{n} = 0;      // driven by the host (Pmod/RGB input)"
                );
            }
            (Dir::In, None) => {
                let l = n.to_ascii_lowercase();
                if l.contains("clk") || l.contains("clock") {
                    let _ = writeln!(s, "    wire {w}{n} = aclk;   // other clock: tied to FCLK0");
                } else {
                    let _ = writeln!(
                        s,
                        "    wire {w}{n} = 0;      // not part of AXI4-Lite: tied low"
                    );
                }
            }
            (Dir::Out, _) => {
                let _ = writeln!(s, "    wire {w}{n};");
            }
            (Dir::InOut, _) => {
                let _ = writeln!(s, "    wire {w}{n};");
            }
        }
    }
    let _ = writeln!(s);

    // Instance.
    let _ = writeln!(s, "    {} dut (", m.name);
    let conns: Vec<String> = m
        .ports
        .iter()
        .map(|p| {
            let net = if p.name == axi.clock {
                "aclk".to_string()
            } else if let Some((r, low)) = &axi.reset {
                if &p.name == r {
                    if *low { "aresetn" } else { "~aresetn" }.to_string()
                } else {
                    p.name.clone()
                }
            } else {
                p.name.clone()
            };
            format!("        .{}({net})", p.name)
        })
        .collect();
    let _ = writeln!(s, "{}\n    );\n", conns.join(",\n"));

    let g = |r: &str| sig(r).unwrap_or_default();
    let aw = axi.addr_width;
    // Pmod and RGB pin groups: outputs are reported to the host on change,
    // inputs follow the levels the host sets (sampled every microsecond).
    let mut pin_blocks = String::new();
    let mut xchg_init = String::new();
    for g in &pin_groups {
        let n = &g.name;
        if g.output {
            // An output with an input port on the same group: the board's
            // devices answer each change at once (ADR-009).
            if let Some(i) = pin_groups.iter().find(|i| !i.output && i.group == g.group) {
                let _ = write!(
                    pin_blocks,
                    "\n    // {n} <-> {inp}: pin group {gr}, answered by the board at once\n\
                     \x20   always @({n}) if (connected) {inp} <= zs_pin_xchg({gr}, {n}, $time);\n",
                    inp = i.name,
                    gr = g.group
                );
                let _ = writeln!(
                    xchg_init,
                    "        {} <= zs_pin_xchg({}, {n}, $time);",
                    i.name, g.group
                );
            } else {
                let _ = write!(
                    pin_blocks,
                    "\n    // {n} -> pin group {gr}\n\
                     \x20   always @({n}) if (connected) zs_pin_out({gr}, {n}, $time);\n",
                    gr = g.group
                );
            }
        } else if g.paired {
            // Driven by the exchange of the output port of the same group.
        } else {
            let _ = write!(
                pin_blocks,
                "\n    // {n} <- pin group {gr}\n\
                 \x20   always #1000 {n} <= zs_pin_in({gr});\n",
                gr = g.group
            );
        }
    }
    // Interrupt outputs go to the PS as IRQ_F2P[i] (level, on change).
    let mut irq_blocks = String::new();
    let mut irq_init = String::new();
    for (k, n) in irqs.iter().enumerate() {
        let bit = match irq_bits {
            None => Some(k as u32),
            Some(b) => b.get(k).copied().flatten(),
        };
        let Some(i) = bit else { continue };
        let _ = write!(
            irq_blocks,
            "\n    // {n} -> IRQ_F2P[{i}]\n\
             \x20   reg irq{i}_q = 1'b0;\n\
             \x20   always @(posedge aclk) begin\n\
             \x20       if (connected && ({n} !== irq{i}_q)) begin\n\
             \x20           irq{i}_q <= {n};\n\
             \x20           rp_irq($time, {i}, {n} ? 1 : 0);\n\
             \x20       end\n\
             \x20   end\n"
        );
        let _ = writeln!(irq_init, "        rp_irq($time, {i}, 0);");
    }
    let _ = write!(
        s,
        "\
{pin_imports}    reg        connected = 1'b0;
    reg        wr_ok;
    reg        rd_ok;
    reg [31:0] rd_val;
{irq_blocks}{pin_blocks}
    task lite_write(input [31:0] a, input [31:0] d, input [3:0] st, output ok);
        begin
            @(posedge aclk);
            {awaddr} <= a[{aw_hi}:0]; {wdata} <= d; {wstrb} <= st;
            {awvalid} <= 1'b1; {wvalid} <= 1'b1;
            @(posedge aclk);
            while (!({awready} && {wready})) @(posedge aclk);
            {awvalid} <= 1'b0; {wvalid} <= 1'b0; {bready} <= 1'b1;
            @(posedge aclk);
            while (!{bvalid}) @(posedge aclk);
            ok = ({bresp} == 2'b00);
            {bready} <= 1'b0;
        end
    endtask

    task lite_read(input [31:0] a, output [31:0] d, output ok);
        begin
            @(posedge aclk);
            {araddr} <= a[{aw_hi}:0]; {arvalid} <= 1'b1;
            @(posedge aclk);
            while (!{arready}) @(posedge aclk);
            {arvalid} <= 1'b0; {rready} <= 1'b1;
            @(posedge aclk);
            while (!{rvalid}) @(posedge aclk);
            d = {rdata}; ok = ({rresp} == 2'b00);
            {rready} <= 1'b0;
        end
    endtask

{master}
{checkers}
    integer a_i, wd_i, ws_i, iw_i, st;
    longint tgt, rts;

    initial begin
        repeat (20) @(posedge aclk);
        aresetn = 1'b1;
        repeat (5) @(posedge aclk);
        rp_lockstep(1);
        if (rp_open(\"\") != 0) begin
            $display(\"cosim_top: rp_open failed (is RP_SOCKET set and QEMU running?)\");
            $finish;
        end
        $display(\"cosim_top: remote-port connected\");
        connected = 1'b1;
{xchg_init}{irq_init}
        forever begin
            {rp_next_call}
            if (st == 0) begin
                $display(\"cosim_top: peer closed, finishing at %0t\", $time);
                rp_close();
                $finish;
            end else if (st == 3) begin
                tgt = rp_sync_target();
                if (tgt > $time) #(tgt - $time);
                rp_sync_ack($time);
            end else if (st == 2) begin
                repeat (20) @(posedge aclk);
            end else begin
                rts = rp_req_ts();
                if (rts > $time) #(rts - $time);
                if (iw_i != 0 && a_i[7:0] == 8'h3C && wd_i == 32'hC051D0DE) begin
                    $display(\"cosim_top: end-of-test marker at %0t\", $time);
                    rp_done_t(0, 1, $time);
                    rp_close();
                    $finish;
                end else if (iw_i != 0) begin
                    lite_write(a_i, wd_i, ws_i[3:0], wr_ok);
                    rp_done_t(0, wr_ok, $time);
                end else begin
                    lite_read(a_i, rd_val, rd_ok);
                    rp_done_t(rd_val, rd_ok, $time);
                end
            end
        end
    end
endmodule
",
        master = master.as_ref().map(master_bridge).unwrap_or_default(),
        pin_imports = PIN_IMPORTS,
        rp_next_call = RP_NEXT_CALL,
        checkers = protocol_checkers(m, axi, master.as_ref()),
        irq_blocks = irq_blocks,
        pin_blocks = pin_blocks,
        xchg_init = xchg_init,
        irq_init = irq_init,
        awaddr = g("awaddr"),
        wdata = g("wdata"),
        wstrb = g("wstrb"),
        awvalid = g("awvalid"),
        wvalid = g("wvalid"),
        awready = g("awready"),
        wready = g("wready"),
        bready = g("bready"),
        bvalid = g("bvalid"),
        bresp = g("bresp"),
        araddr = g("araddr"),
        arvalid = g("arvalid"),
        arready = g("arready"),
        rready = g("rready"),
        rvalid = g("rvalid"),
        rdata = g("rdata"),
        rresp = g("rresp"),
        aw_hi = aw - 1,
    );
    s
}

/// One slave descriptor for multi-slave testbench generation.
#[derive(Clone, Debug)]
pub struct SlaveDesc<'a> {
    /// Instance name in the block design / testbench, e.g. `u_math0`.
    pub instance: &'a str,
    /// Parsed module info.
    pub module: &'a ModuleInfo,
    /// AXI4-Lite slave interface.
    pub axi: &'a AxiLite,
    /// Base address of the slave.
    pub base: u64,
    /// Size of the address window in bytes.
    pub size: u64,
    /// IRQ bits for the interrupt outputs found by `detect_irqs`.
    pub irq_bits: &'a [Option<u32>],
    /// Master interface if the module acts as a bus master, and its remote-port device number (e.g. 2 for S_AXI_HP0).
    pub master: Option<(&'a AxiMaster, u32)>,
}

/// Generates a testbench instantiating multiple AXI4-Lite slaves, an address decoder
/// routing `M_AXI_GP0` accesses to each slave by address range, and optional AXI4 master
/// bridges connecting to PS memory.
pub fn generate_multi_slave_testbench(
    slaves: &[SlaveDesc<'_>],
    clock_hz: Option<u64>,
) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "`timescale 1ns / 1ps\n\
         // Generated by `zynqsim init --xsa`: stands in for the PS7 and drives {} user RTL slave(s)\n\
         // from the remote-port link (lock-step).\n\
         // Edit freely: it is a plain testbench. A write of 0xC051D0DE to offset 0x3C ends the run.\n\
         module cosim_top;",
        slaves.len()
    );
    s.push_str(RP_IMPORTS);
    let _ = writeln!(s);

    let (half_str, mhz_comment) = match clock_hz {
        Some(hz) if hz != 100_000_000 => {
            let half_ns = 5.0e8 / hz as f64;
            let half = format!("{half_ns:.3}")
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string();
            (half, format!("// {} MHz, from the block design", hz as f64 / 1e6))
        }
        _ => ("5".to_string(), "// 100 MHz = FCLK_CLK0".to_string()),
    };

    let _ = writeln!(
        s,
        "    reg aclk = 1'b0;\n    reg aresetn = 1'b0;\n    always #{half_str} aclk = ~aclk;   {mhz_comment}\n"
    );

    let mut pin_blocks = String::new();
    let mut xchg_init = String::new();
    let mut irq_blocks = String::new();
    let mut irq_init = String::new();
    let mut master_blocks = String::new();
    let mut checker_blocks = String::new();
    let mut write_tasks = String::new();
    let mut read_tasks = String::new();
    let mut write_decoder = String::new();
    let mut read_decoder = String::new();

    let mut chk_id = 0usize;

    for (idx, slv) in slaves.iter().enumerate() {
        let pfx = slv.instance;
        let pin_groups = detect_pin_groups(slv.module);
        let mut role_of: HashMap<&str, &str> =
            slv.axi.signals.iter().map(|(r, n)| (n.as_str(), *r)).collect();
        if let Some((mst, _)) = &slv.master {
            for (r, n) in &mst.signals {
                role_of.insert(n.as_str(), *r);
            }
        }

        // Port signal declarations
        let _ = writeln!(s, "    // ---- Slave {idx}: {} ({}) ----", pfx, slv.module.name);
        for p in &slv.module.ports {
            let n = &p.name;
            if *n == slv.axi.clock || slv.axi.reset.as_ref().is_some_and(|(r, _)| r == n) {
                continue;
            }
            let w = decl(p.width);
            let net = format!("{pfx}_{n}");
            match (p.dir, role_of.get(n.as_str())) {
                (Dir::In, Some(_)) => {
                    let _ = writeln!(s, "    reg  {w}{net} = 0;");
                }
                (Dir::In, None) if pin_groups.iter().any(|g| &g.name == n && g.paired) => {
                    let _ = writeln!(
                        s,
                        "    reg  {w}{net} = '1;     // driven by the board's devices (released = 1)"
                    );
                }
                (Dir::In, None) if pin_groups.iter().any(|g| &g.name == n) => {
                    let _ = writeln!(
                        s,
                        "    reg  {w}{net} = 0;      // driven by the host (Pmod/RGB input)"
                    );
                }
                (Dir::In, None) => {
                    let l = n.to_ascii_lowercase();
                    if l.contains("clk") || l.contains("clock") {
                        let _ = writeln!(s, "    wire {w}{net} = aclk;   // other clock: tied to FCLK0");
                    } else {
                        let _ = writeln!(
                            s,
                            "    wire {w}{net} = 0;      // not part of AXI4-Lite: tied low"
                        );
                    }
                }
                (Dir::Out, _) | (Dir::InOut, _) => {
                    let _ = writeln!(s, "    wire {w}{net};");
                }
            }
        }
        let _ = writeln!(s);

        // Instance
        let _ = writeln!(s, "    {} {} (", slv.module.name, pfx);
        let conns: Vec<String> = slv
            .module
            .ports
            .iter()
            .map(|p| {
                let net = if p.name == slv.axi.clock {
                    "aclk".to_string()
                } else if let Some((r, low)) = &slv.axi.reset {
                    if &p.name == r {
                        if *low { "aresetn" } else { "~aresetn" }.to_string()
                    } else {
                        format!("{pfx}_{}", p.name)
                    }
                } else {
                    format!("{pfx}_{}", p.name)
                };
                format!("        .{}({net})", p.name)
            })
            .collect();
        let _ = writeln!(s, "{}\n    );\n", conns.join(",\n"));

        // Pin groups
        for g in &pin_groups {
            let n = format!("{pfx}_{}", g.name);
            if g.output {
                if let Some(i) = pin_groups.iter().find(|i| !i.output && i.group == g.group) {
                    let inp = format!("{pfx}_{}", i.name);
                    let _ = write!(
                        pin_blocks,
                        "\n    // {n} <-> {inp}: pin group {gr}, answered by the board at once\n\
                         \x20   always @({n}) if (connected) {inp} <= zs_pin_xchg({gr}, {n}, $time);\n",
                        gr = g.group
                    );
                    let _ = writeln!(
                        xchg_init,
                        "        {} <= zs_pin_xchg({}, {n}, $time);",
                        inp, g.group
                    );
                } else {
                    let _ = write!(
                        pin_blocks,
                        "\n    // {n} -> pin group {gr}\n\
                         \x20   always @({n}) if (connected) zs_pin_out({gr}, {n}, $time);\n",
                        gr = g.group
                    );
                }
            } else if !g.paired {
                let _ = write!(
                    pin_blocks,
                    "\n    // {n} <- pin group {gr}\n\
                     \x20   always #1000 {n} <= zs_pin_in({gr});\n",
                    gr = g.group
                );
            }
        }

        // Interrupts
        let irqs = detect_irqs(slv.module, slv.axi);
        for (k, n) in irqs.iter().enumerate() {
            let bit = slv.irq_bits.get(k).copied().flatten();
            let Some(i) = bit else { continue };
            let sig_name = format!("{pfx}_{n}");
            let _ = write!(
                irq_blocks,
                "\n    // {pfx}.{n} -> IRQ_F2P[{i}]\n\
                 \x20   reg {pfx}_irq{i}_q = 1'b0;\n\
                 \x20   always @(posedge aclk) begin\n\
                 \x20       if (connected && ({sig_name} !== {pfx}_irq{i}_q)) begin\n\
                 \x20           {pfx}_irq{i}_q <= {sig_name};\n\
                 \x20           rp_irq($time, {i}, {sig_name} ? 1 : 0);\n\
                 \x20       end\n\
                 \x20   end\n"
            );
            let _ = writeln!(irq_init, "        rp_irq($time, {i}, 0);");
        }

        // Tasks
        let g_sig = |r: &str| format!("{pfx}_{}", slv.axi.signals[r]);
        let aw_hi = slv.axi.addr_width - 1;
        let _ = write!(
            write_tasks,
            "    task lite_write_{idx}(input [31:0] a, input [31:0] d, input [3:0] st, output ok);\n\
             \x20       begin\n\
             \x20           @(posedge aclk);\n\
             \x20           {awaddr} <= a[{aw_hi}:0]; {wdata} <= d; {wstrb} <= st;\n\
             \x20           {awvalid} <= 1'b1; {wvalid} <= 1'b1;\n\
             \x20           @(posedge aclk);\n\
             \x20           while (!({awready} && {wready})) @(posedge aclk);\n\
             \x20           {awvalid} <= 1'b0; {wvalid} <= 1'b0; {bready} <= 1'b1;\n\
             \x20           @(posedge aclk);\n\
             \x20           while (!{bvalid}) @(posedge aclk);\n\
             \x20           ok = ({bresp} == 2'b00);\n\
             \x20           {bready} <= 1'b0;\n\
             \x20       end\n\
             \x20   endtask\n\n",
            awaddr = g_sig("awaddr"),
            wdata = g_sig("wdata"),
            wstrb = g_sig("wstrb"),
            awvalid = g_sig("awvalid"),
            wvalid = g_sig("wvalid"),
            awready = g_sig("awready"),
            wready = g_sig("wready"),
            bready = g_sig("bready"),
            bvalid = g_sig("bvalid"),
            bresp = g_sig("bresp"),
            aw_hi = aw_hi
        );

        let _ = write!(
            read_tasks,
            "    task lite_read_{idx}(input [31:0] a, output [31:0] d, output ok);\n\
             \x20       begin\n\
             \x20           @(posedge aclk);\n\
             \x20           {araddr} <= a[{aw_hi}:0]; {arvalid} <= 1'b1;\n\
             \x20           @(posedge aclk);\n\
             \x20           while (!{arready}) @(posedge aclk);\n\
             \x20           {arvalid} <= 1'b0; {rready} <= 1'b1;\n\
             \x20           @(posedge aclk);\n\
             \x20           while (!{rvalid}) @(posedge aclk);\n\
             \x20           d = {rdata}; ok = ({rresp} == 2'b00);\n\
             \x20           {rready} <= 1'b0;\n\
             \x20       end\n\
             \x20   endtask\n\n",
            araddr = g_sig("araddr"),
            arvalid = g_sig("arvalid"),
            arready = g_sig("arready"),
            rready = g_sig("rready"),
            rvalid = g_sig("rvalid"),
            rdata = g_sig("rdata"),
            rresp = g_sig("rresp"),
            aw_hi = aw_hi
        );

        // Master bridge
        if let Some((mst, dev)) = &slv.master {
            master_blocks.push_str(&master_bridge_with_dev(mst, *dev, pfx));
        }

        // Protocol checkers
        let dw = slv
            .axi
            .signals
            .get("wdata")
            .and_then(|n| port_width(slv.module, n))
            .unwrap_or(32);
        let _ = write!(
            checker_blocks,
            "    zs_axi_checker #(.ID({chk_id}), .FULL(0), .AW({aw}), .DW({dw})) zs_chk_lite_{idx} (\n\
             \x20       .aclk(aclk), .aresetn(aresetn),\n\
             \x20       .awvalid({awvalid}), .awready({awready}), .awaddr({awaddr}),\n\
             \x20       .awlen(8'd0), .awsize(3'd2), .awburst(2'd1),\n\
             \x20       .wvalid({wvalid}), .wready({wready}), .wdata({wdata}), .wstrb({wstrb}), .wlast(1'b1),\n\
             \x20       .bvalid({bvalid}), .bready({bready}), .bresp({bresp}),\n\
             \x20       .arvalid({arvalid}), .arready({arready}), .araddr({araddr}),\n\
             \x20       .arlen(8'd0), .arsize(3'd2), .arburst(2'd1),\n\
             \x20       .rvalid({rvalid}), .rready({rready}), .rdata({rdata}), .rresp({rresp}), .rlast(1'b1)\n\
             \x20   );\n\n",
            chk_id = chk_id,
            aw = slv.axi.addr_width,
            dw = dw,
            awvalid = g_sig("awvalid"),
            awready = g_sig("awready"),
            awaddr = g_sig("awaddr"),
            wvalid = g_sig("wvalid"),
            wready = g_sig("wready"),
            wdata = g_sig("wdata"),
            wstrb = g_sig("wstrb"),
            bvalid = g_sig("bvalid"),
            bready = g_sig("bready"),
            bresp = g_sig("bresp"),
            arvalid = g_sig("arvalid"),
            arready = g_sig("arready"),
            araddr = g_sig("araddr"),
            rvalid = g_sig("rvalid"),
            rready = g_sig("rready"),
            rdata = g_sig("rdata"),
            rresp = g_sig("rresp"),
        );
        chk_id += 1;

        if let Some((mst, _)) = &slv.master {
            let m_sig = |r: &str| format!("{pfx}_{}", mst.signals[r]);
            let maw = port_width(slv.module, &mst.signals["awaddr"]).unwrap_or(32);
            let mdw = port_width(slv.module, &mst.signals["wdata"]).unwrap_or(32);
            let _ = write!(
                checker_blocks,
                "    zs_axi_checker #(.ID({chk_id}), .FULL(1), .AW({maw}), .DW({mdw})) zs_chk_mst_{idx} (\n\
                 \x20       .aclk(aclk), .aresetn(aresetn),\n\
                 \x20       .awvalid({awvalid}), .awready({awready}), .awaddr({awaddr}),\n\
                 \x20       .awlen({awlen}), .awsize({awsize}), .awburst({awburst}),\n\
                 \x20       .wvalid({wvalid}), .wready({wready}), .wdata({wdata}), .wstrb({wstrb}), .wlast({wlast}),\n\
                 \x20       .bvalid({bvalid}), .bready({bready}), .bresp({bresp}),\n\
                 \x20       .arvalid({arvalid}), .arready({arready}), .araddr({araddr}),\n\
                 \x20       .arlen({arlen}), .arsize({arsize}), .arburst({arburst}),\n\
                 \x20       .rvalid({rvalid}), .rready({rready}), .rdata({rdata}), .rresp({rresp}), .rlast({rlast})\n\
                 \x20   );\n\n",
                chk_id = chk_id,
                maw = maw,
                mdw = mdw,
                awvalid = m_sig("awvalid"),
                awready = m_sig("awready"),
                awaddr = m_sig("awaddr"),
                awlen = m_sig("awlen"),
                awsize = m_sig("awsize"),
                awburst = m_sig("awburst"),
                wvalid = m_sig("wvalid"),
                wready = m_sig("wready"),
                wdata = m_sig("wdata"),
                wstrb = m_sig("wstrb"),
                wlast = m_sig("wlast"),
                bvalid = m_sig("bvalid"),
                bready = m_sig("bready"),
                bresp = m_sig("bresp"),
                arvalid = m_sig("arvalid"),
                arready = m_sig("arready"),
                araddr = m_sig("araddr"),
                arlen = m_sig("arlen"),
                arsize = m_sig("arsize"),
                arburst = m_sig("arburst"),
                rvalid = m_sig("rvalid"),
                rready = m_sig("rready"),
                rdata = m_sig("rdata"),
                rresp = m_sig("rresp"),
                rlast = m_sig("rlast"),
            );
            chk_id += 1;
        }

        // Decoders
        let base = slv.base;
        let high = slv.base + slv.size.max(1) - 1;
        let prefix_cond = if idx == 0 { "if" } else { "else if" };
        let _ = write!(
            write_decoder,
            "                    {prefix_cond} (u_addr >= 32'h{base:08X} && u_addr <= 32'h{high:08X}) begin\n\
             \x20                       lite_write_{idx}(u_addr - 32'h{base:08X}, wd_i, ws_i[3:0], wr_ok);\n\
             \x20                       rp_done_t(0, wr_ok, $time);\n\
             \x20                   end "
        );
        let _ = write!(
            read_decoder,
            "                    {prefix_cond} (u_addr >= 32'h{base:08X} && u_addr <= 32'h{high:08X}) begin\n\
             \x20                       lite_read_{idx}(u_addr - 32'h{base:08X}, rd_val, rd_ok);\n\
             \x20                       rp_done_t(rd_val, rd_ok, $time);\n\
             \x20                   end "
        );
    }

    let _ = write!(
        write_decoder,
        "else begin\n\
         \x20                       $display(\"cosim_top: unmapped write at %08x\", u_addr);\n\
         \x20                       rp_done_t(0, 0, $time);\n\
         \x20                   end"
    );
    let _ = write!(
        read_decoder,
        "else begin\n\
         \x20                       $display(\"cosim_top: unmapped read at %08x\", u_addr);\n\
         \x20                       rp_done_t(32'hDEC0_DE00, 0, $time);\n\
         \x20                   end"
    );

    let _ = write!(
        s,
        "{pin_imports}    reg        connected = 1'b0;\n\
         \x20   reg        wr_ok;\n\
         \x20   reg        rd_ok;\n\
         \x20   reg [31:0] rd_val;\n\
         {irq_blocks}{pin_blocks}\n\
         {write_tasks}\
         {read_tasks}\
         {master_blocks}\
         `ifndef ZS_NO_PROTOCOL_CHECKERS\n\
         \x20   // AXI protocol checkers (ADR-025): they observe the bus and drive nothing.\n\
         {checker_blocks}\
         `endif\n\n\
         \x20   integer a_i, wd_i, ws_i, iw_i, st;\n\
         \x20   longint tgt, rts;\n\
         \x20   wire [31:0] u_addr = a_i;\n\n\
         \x20   initial begin\n\
         \x20       repeat (20) @(posedge aclk);\n\
         \x20       aresetn = 1'b1;\n\
         \x20       repeat (5) @(posedge aclk);\n\
         \x20       rp_lockstep(1);\n\
         \x20       if (rp_open(\"\") != 0) begin\n\
         \x20           $display(\"cosim_top: rp_open failed (is RP_SOCKET set and QEMU running?)\");\n\
         \x20           $finish;\n\
         \x20       end\n\
         \x20       $display(\"cosim_top: remote-port connected\");\n\
         \x20       connected = 1'b1;\n\
         {xchg_init}{irq_init}\
         \x20       forever begin\n\
         \x20           {rp_next_call}\
         \x20           if (st == 0) begin\n\
         \x20               $display(\"cosim_top: peer closed, finishing at %0t\", $time);\n\
         \x20               rp_close();\n\
         \x20               $finish;\n\
         \x20           end else if (st == 3) begin\n\
         \x20               tgt = rp_sync_target();\n\
         \x20               if (tgt > $time) #(tgt - $time);\n\
         \x20               rp_sync_ack($time);\n\
         \x20           end else if (st == 2) begin\n\
         \x20               repeat (20) @(posedge aclk);\n\
         \x20           end else begin\n\
         \x20               rts = rp_req_ts();\n\
         \x20               if (rts > $time) #(rts - $time);\n\
         \x20               if (iw_i != 0 && a_i[7:0] == 8'h3C && wd_i == 32'hC051D0DE) begin\n\
         \x20                   $display(\"cosim_top: end-of-test marker at %0t\", $time);\n\
         \x20                   rp_done_t(0, 1, $time);\n\
         \x20                   rp_close();\n\
         \x20                   $finish;\n\
         \x20               end else if (iw_i != 0) begin\n\
         {write_decoder}\n\
         \x20               end else begin\n\
         {read_decoder}\n\
         \x20               end\n\
         \x20           end\n\
         \x20       end\n\
         \x20   end\n\
         endmodule\n",
        pin_imports = PIN_IMPORTS,
        irq_blocks = irq_blocks,
        pin_blocks = pin_blocks,
        write_tasks = write_tasks,
        read_tasks = read_tasks,
        master_blocks = master_blocks,
        checker_blocks = checker_blocks,
        xchg_init = xchg_init,
        irq_init = irq_init,
        rp_next_call = RP_NEXT_CALL,
        write_decoder = write_decoder,
        read_decoder = read_decoder,
    );
    s
}
fn strip_verbatim(p: std::path::PathBuf) -> std::path::PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => std::path::PathBuf::from(rest),
        _ => p,
    }
}

/// `to` relative to the directory `from` (both made absolute first), with
/// forward slashes; `to` itself if they share no root (other drive).
pub fn relative_path(from: &std::path::Path, to: &std::path::Path) -> String {
    // Resolve symbolic links of the part that exists (macOS: /var is
    // /private/var, so a temp dir and a checkout would not share a root);
    // the rest is appended as given. Both sides get the same treatment, so a
    // Windows `\?\` prefix cancels out.
    let abs = |p: &std::path::Path| {
        let a = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
        let mut existing = a.clone();
        let mut tail: Vec<std::ffi::OsString> = Vec::new();
        loop {
            if let Ok(real) = existing.canonicalize() {
                let mut out = strip_verbatim(real);
                out.extend(tail.iter().rev());
                return out;
            }
            match (
                existing.file_name().map(|n| n.to_os_string()),
                existing.parent(),
            ) {
                (Some(n), Some(parent)) => {
                    tail.push(n);
                    existing = parent.to_path_buf();
                }
                _ => return a,
            }
        }
    };
    let (f, t) = (abs(from), abs(to));
    let fc: Vec<_> = f.components().collect();
    let tc: Vec<_> = t.components().collect();
    let common = fc.iter().zip(&tc).take_while(|(a, b)| a == b).count();
    if common == 0 {
        return t.to_string_lossy().replace('\\', "/");
    }
    let mut out = std::path::PathBuf::new();
    for _ in common..fc.len() {
        out.push("..");
    }
    for c in &tc[common..] {
        out.push(c.as_os_str());
    }
    out.to_string_lossy().replace('\\', "/")
}

/// A `project.toml` for the generated testbench. Paths are as given
/// (relative to the project directory).
pub fn generate_project(
    rtl_files: &[String],
    testbench: &str,
    elf: &str,
    elf1: Option<&str>,
    ip_name: &str,
    base_hint: u64,
) -> String {
    // Paths go into TOML basic strings: forward slashes (valid on Windows
    // too) and no quotes, so a Windows path cannot break the file.
    let esc = |p: &str| p.replace('\\', "/").replace('"', "");
    let elf = esc(elf);
    let cpu1 = elf1
        .map(|e| format!("\n[cpu1]\nelf = \"{}\"\n", esc(e)))
        .unwrap_or_default();
    let files: Vec<String> = rtl_files
        .iter()
        .map(|f| f.as_str())
        .chain(std::iter::once(testbench))
        .map(|f| format!("    \"{}\",", esc(f)))
        .collect();
    format!(
        "# Generated by `zynqsim init --ip`. `{ip_name}` answers every PS access to the\n\
         # M_AXI_GP0 window; the testbench ignores the upper address bits, so use the address your\n\
         # block design assigns (for example {base_hint:#010x}) and offsets inside the IP.\n\
         schema_version = 1\n\
         \n\
         [board]\n\
         name = \"zybo-z7-20\"\n\
         \n\
         [backends]\n\
         ps = \"qemu\"\n\
         pl = \"xsim\"\n\
         \n\
         [sim]\n\
         quantum_ns = 1000\n\
         icount_shift = 1\n\
         \n\
         [boot]\n\
         mode = \"elf\"\n\
         \n\
         [cpu0]\n\
         elf = \"{elf}\"\n\
         {cpu1}\n\
         [rtl]\n\
         files = [\n{}\n]\n\
         top = \"cosim_top\"\n\
         # waves = true   # keep the XSim waveform (run directory under %TEMP%\\zynqsim)\n\
         # protocol_checkers = \"strict\"   # AXI checkers (ADR-025): true (warn), false, \"strict\"\n\
         \n\
         # Uncomment to stop the CPU at breakpoints and read its registers:\n\
         # [debug]\n\
         # enabled = true\n\
         # breakpoints = [\"main\"]\n",
        files.join("\n"),
        cpu1 = cpu1.trim_start_matches('\n'),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> String {
        std::fs::read_to_string(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/rtl")
                .join(rel),
        )
        .unwrap()
    }

    #[test]
    fn the_math_ip_header_is_understood() {
        let m = parse_module(&fixture("math/my_math_ip.v"), None).unwrap();
        assert_eq!(m.name, "my_math_ip");
        assert_eq!(m.ports.len(), 21);
        let by = |n: &str| m.ports.iter().find(|p| p.name == n).unwrap();
        assert_eq!(by("S_AXI_AWADDR").width, 4, "parameter arithmetic");
        assert_eq!(by("S_AXI_WSTRB").width, 4, "(32/8)-1");
        assert_eq!(by("S_AXI_RDATA").dir, Dir::Out);
        assert_eq!(by("S_AXI_ACLK").width, 1);

        let axi = detect_axi_lite(&m).unwrap();
        assert_eq!(axi.prefix, "S_AXI_");
        assert_eq!(axi.addr_width, 4);
        assert_eq!(axi.clock, "S_AXI_ACLK");
        assert_eq!(axi.reset, Some(("S_AXI_ARESETN".to_string(), true)));
        assert_eq!(axi.signals["wdata"], "S_AXI_WDATA");
    }

    #[test]
    fn the_generated_testbench_wires_every_port() {
        let m = parse_module(&fixture("math/my_math_ip.v"), Some("my_math_ip")).unwrap();
        let axi = detect_axi_lite(&m).unwrap();
        let tb = generate_testbench(&m, &axi);
        assert!(tb.contains("my_math_ip dut ("));
        assert!(tb.contains(".S_AXI_ACLK(aclk)"));
        assert!(tb.contains(".S_AXI_ARESETN(aresetn)"));
        assert!(tb.contains("reg  [3:0] S_AXI_AWADDR = 0;"), "{tb}");
        assert!(tb.contains("wire [31:0] S_AXI_RDATA;"));
        assert!(tb.contains("S_AXI_AWADDR <= a[3:0];"));
        // Every port of the module appears in the instance.
        for p in &m.ports {
            assert!(tb.contains(&format!(".{}(", p.name)), "{}", p.name);
        }
        assert!(tb.contains("rp_lockstep(1)"));
    }

    #[test]
    fn the_icarus_form_wraps_every_dpi_import_and_the_dpi_form_is_kept() {
        // Icarus Verilog (ADR-037): under ZS_ICARUS every DPI function the testbench calls is
        // a wrapper of `$zs_<name>`; without it the `import "DPI-C"` lines are the ones the
        // DPI simulators take, as before.
        let m = parse_module(&fixture("math/my_math_ip.v"), None).unwrap();
        let axi = detect_axi_lite(&m).unwrap();
        let tb = generate_testbench(&m, &axi);
        let (icarus, dpi) = tb.split_once("`else").expect("the ZS_ICARUS branch");
        let icarus = icarus.split_once("`ifdef ZS_ICARUS").expect("the guard").1;
        let dpi = dpi.split_once("`endif").expect("the end of the guard").0;
        assert!(dpi.contains("import \"DPI-C\" function int  rp_open(input string descr);"));
        assert!(
            dpi.contains("import \"DPI-C\" function int  rp_next("),
            "rp_next keeps its import"
        );
        assert!(
            !icarus.contains("import \"DPI-C\""),
            "no DPI-C in the Icarus branch"
        );
        for name in [
            "rp_open",
            "rp_done_t",
            "rp_close",
            "rp_lockstep",
            "rp_sync_target",
            "rp_sync_ack",
            "rp_req_ts",
            "rp_irq",
            "rp_mem_put",
            "rp_mem_start",
            "rp_mem_wait",
            "rp_mem_get",
            "rp_mem_free",
        ] {
            assert!(
                icarus.contains(&format!("$zs_{name}(")),
                "{name} has no $zs_ call"
            );
        }
        // rp_next has output arguments: its call site calls the system function instead.
        assert!(
            tb.contains("st = $zs_rp_next(zs_now, -1, a_i, wd_i, ws_i, iw_i);"),
            "{tb}"
        );
        assert!(
            tb.contains("st = rp_next($time, -1, a_i, wd_i, ws_i, iw_i);"),
            "{tb}"
        );
        // The pin imports of the testbench have the same two forms.
        assert!(
            tb.contains("$zs_pin_out(group, levels, now_ns);"),
            "the pin wrapper is written"
        );
        assert!(
            tb.contains("import \"DPI-C\" function void zs_pin_out("),
            "{tb}"
        );
        let opened = tb.matches("`ifdef").count() + tb.matches("`ifndef").count();
        assert_eq!(tb.matches("`endif").count(), opened, "{tb}");
    }

    #[test]
    fn lowercase_names_active_high_reset_and_extra_ports() {
        let src = "
            module ip #(parameter AW = 6) (
                input  wire clk,
                input  wire rst,
                input  wire [AW-1:0] s00_axi_awaddr, input s00_axi_awvalid, output s00_axi_awready,
                input  [31:0] s00_axi_wdata, input [3:0] s00_axi_wstrb,
                input s00_axi_wvalid, output s00_axi_wready,
                output [1:0] s00_axi_bresp, output s00_axi_bvalid, input s00_axi_bready,
                input [AW-1:0] s00_axi_araddr, input s00_axi_arvalid, output s00_axi_arready,
                output [31:0] s00_axi_rdata, output [1:0] s00_axi_rresp,
                output s00_axi_rvalid, input s00_axi_rready,
                input mclk, input [7:0] gpio_in, output [7:0] gpio_out
            );
            endmodule";
        let m = parse_module(src, None).unwrap();
        let axi = detect_axi_lite(&m).unwrap();
        assert_eq!(axi.prefix, "s00_axi_");
        assert_eq!(axi.addr_width, 6);
        assert_eq!(axi.clock, "clk");
        assert_eq!(axi.reset, Some(("rst".to_string(), false)), "active high");
        let tb = generate_testbench(&m, &axi);
        assert!(tb.contains(".rst(~aresetn)"));
        assert!(
            tb.contains("wire  mclk = aclk;") || tb.contains("wire mclk = aclk;"),
            "{tb}"
        );
        assert!(tb.contains("wire [7:0] gpio_in = 0;"));
        assert!(tb.contains("wire [7:0] gpio_out;"));
    }

    #[test]
    fn interrupt_outputs_are_forwarded_as_irq_f2p() {
        let src = fixture("irq/irq_ip.v");
        let m = parse_module(&src, None).unwrap();
        let axi = detect_axi_lite(&m).unwrap();
        assert_eq!(detect_irqs(&m, &axi), vec!["irq".to_string()]);
        let tb = generate_testbench(&m, &axi);
        assert!(tb.contains("// irq -> IRQ_F2P[0]"), "{tb}");
        assert!(tb.contains("rp_irq($time, 0, irq ? 1 : 0);"));
        assert!(tb.contains("connected = 1'b1;"));
        // The math IP has none.
        let m = parse_module(&fixture("math/my_math_ip.v"), None).unwrap();
        let axi = detect_axi_lite(&m).unwrap();
        assert!(detect_irqs(&m, &axi).is_empty());
        assert!(!generate_testbench(&m, &axi).contains("IRQ_F2P"));
    }

    #[test]
    fn pmod_and_rgb_ports_become_pin_groups() {
        let m = parse_module(&fixture("pmod/oled_spi.v"), None).unwrap();
        let axi = detect_axi_lite(&m).unwrap();
        let g = detect_pin_groups(&m);
        assert_eq!(
            g,
            vec![PinGroup {
                name: "pmod_ja".into(),
                group: 0,
                output: true,
                paired: false
            }]
        );
        let tb = generate_testbench(&m, &axi);
        assert!(
            tb.contains("always @(pmod_ja) if (connected) zs_pin_out(0, pmod_ja, $time);"),
            "{tb}"
        );

        let src =
            "module b (input clk, input [7:0] pmod_jb_i, output [7:0] jc_o, output [5:0] rgb_leds,
                   input [3:0] s_awaddr, input s_awvalid, output s_awready, input [31:0] s_wdata,
                   input [3:0] s_wstrb, input s_wvalid, output s_wready, output [1:0] s_bresp,
                   output s_bvalid, input s_bready, input [3:0] s_araddr, input s_arvalid,
                   output s_arready, output [31:0] s_rdata, output [1:0] s_rresp,
                   output s_rvalid, input s_rready); endmodule";
        let m = parse_module(src, None).unwrap();
        let names: Vec<(String, u8, bool)> = detect_pin_groups(&m)
            .into_iter()
            .map(|g| (g.name, g.group, g.output))
            .collect();
        assert_eq!(
            names,
            vec![
                ("pmod_jb_i".to_string(), 1, false),
                ("jc_o".to_string(), 2, true),
                ("rgb_leds".to_string(), 6, true),
            ]
        );
        let axi = detect_axi_lite(&m).unwrap();
        let tb = generate_testbench(&m, &axi);
        assert!(
            tb.contains("always #1000 pmod_jb_i <= zs_pin_in(1);"),
            "{tb}"
        );
        assert!(tb.contains("reg  [7:0] pmod_jb_i = 0;"), "{tb}");
    }

    #[test]
    fn an_output_and_an_input_on_one_pmod_port_use_the_exchange() {
        let src = "module b (input clk, output [7:0] pmod_ja_o, input [7:0] pmod_ja_i,
                   output [7:0] pmod_jb, input [7:0] pmod_jc,
                   input [3:0] s_awaddr, input s_awvalid, output s_awready, input [31:0] s_wdata,
                   input [3:0] s_wstrb, input s_wvalid, output s_wready, output [1:0] s_bresp,
                   output s_bvalid, input s_bready, input [3:0] s_araddr, input s_arvalid,
                   output s_arready, output [31:0] s_rdata, output [1:0] s_rresp,
                   output s_rvalid, input s_rready); endmodule";
        let m = parse_module(src, None).unwrap();
        let g = detect_pin_groups(&m);
        let paired: Vec<(&str, bool)> = g.iter().map(|g| (g.name.as_str(), g.paired)).collect();
        assert_eq!(
            paired,
            vec![
                ("pmod_ja_o", true),
                ("pmod_ja_i", true),
                ("pmod_jb", false),
                ("pmod_jc", false)
            ]
        );
        let axi = detect_axi_lite(&m).unwrap();
        let tb = generate_testbench(&m, &axi);
        assert!(
            tb.contains(
                "always @(pmod_ja_o) if (connected) pmod_ja_i <= zs_pin_xchg(0, pmod_ja_o, $time);"
            ),
            "{tb}"
        );
        assert!(
            tb.contains("        pmod_ja_i <= zs_pin_xchg(0, pmod_ja_o, $time);\n"),
            "an initial exchange at connect: {tb}"
        );
        assert!(tb.contains("reg  [7:0] pmod_ja_i = '1;"), "{tb}");
        assert!(
            !tb.contains("zs_pin_in(0)"),
            "a paired input is not polled: {tb}"
        );
        assert!(tb.contains("always @(pmod_jb) if (connected) zs_pin_out(1, pmod_jb, $time);"));
        assert!(tb.contains("always #1000 pmod_jc <= zs_pin_in(2);"));
    }

    #[test]
    fn non_ansi_headers_and_comments_parse() {
        let src = "
            // a comment with module in it
            module old_style (aclk, aresetn, awaddr);
              /* block comment */
              input aclk;
              input aresetn;
              input [7:0] awaddr;
            endmodule";
        let m = parse_module(src, Some("old_style")).unwrap();
        assert_eq!(
            m.ports,
            vec![
                Port {
                    name: "aclk".into(),
                    dir: Dir::In,
                    width: 1
                },
                Port {
                    name: "aresetn".into(),
                    dir: Dir::In,
                    width: 1
                },
                Port {
                    name: "awaddr".into(),
                    dir: Dir::In,
                    width: 8
                },
            ]
        );
    }

    #[test]
    fn unsuitable_modules_are_rejected_with_a_reason() {
        let no_axi = "module m (input clk, input [7:0] d, output [7:0] q); endmodule";
        let m = parse_module(no_axi, None).unwrap();
        assert!(detect_axi_lite(&m).unwrap_err().contains("AXI4-Lite"));

        // 64-bit data.
        let wide = fixture("math/my_math_ip.v")
            .replace("C_S_AXI_DATA_WIDTH = 32", "C_S_AXI_DATA_WIDTH = 64");
        let m = parse_module(&wide, None).unwrap();
        assert!(detect_axi_lite(&m).unwrap_err().contains("32"));

        assert!(parse_module("wire x;", None).is_err());
        assert!(
            parse_module("module a (input clk); endmodule", Some("b"))
                .unwrap_err()
                .contains("`b`")
        );
    }

    #[test]
    fn the_second_module_in_a_file_can_be_chosen() {
        let src = "module first (input a); endmodule\nmodule second (input b, output c); endmodule";
        assert_eq!(parse_module(src, None).unwrap().name, "first");
        let m = parse_module(src, Some("second")).unwrap();
        assert_eq!(m.ports.len(), 2);
    }

    #[test]
    fn windows_paths_do_not_break_the_project_file() {
        let toml = generate_project(
            &["C:\\ip dir\\my_ip.v".to_string()],
            "tb/cosim_top.sv",
            "C:\\fw\\app.elf",
            None,
            "my_ip",
            0x43C0_0000,
        );
        let p = crate::Project::from_toml(&toml).expect("valid TOML and schema");
        assert_eq!(p.cpu0.unwrap().elf, "C:/fw/app.elf");
        assert_eq!(p.rtl.unwrap().files[0], "C:/ip dir/my_ip.v");
    }

    #[test]
    fn a_dma_ip_gets_a_slave_a_master_bridge_and_an_interrupt() {
        let m = parse_module(&fixture("dma/dma_xform.v"), None).unwrap();
        let axi = detect_axi_lite(&m).unwrap();
        assert_eq!(axi.prefix, "s_", "the slave, not the master");
        assert_eq!(axi.addr_width, 6);
        let mst = detect_axi_master(&m).unwrap().expect("a master interface");
        assert_eq!(mst.prefix, "m_");
        assert_eq!(mst.signals["araddr"], "m_araddr");
        assert_eq!(detect_irqs(&m, &axi), vec!["irq".to_string()]);

        let tb = generate_testbench(&m, &axi);
        assert!(tb.contains("rp_mem_start($time, HP0_DEV, 0, ra,"), "{tb}");
        assert!(tb.contains("rp_mem_start($time, HP0_DEV, 1, wa,"));
        // The IP's master outputs are wires, the response inputs are regs the
        // bridge drives.
        assert!(tb.contains("wire [31:0] m_araddr;"), "{tb}");
        assert!(
            tb.contains("reg  m_arready = 0;") || tb.contains("reg  m_arready"),
            "{tb}"
        );
        assert!(tb.contains("reg  [31:0] m_rdata = 0;"));
        assert!(tb.contains("s_awaddr <= a[5:0];"));
        assert!(tb.contains("rp_irq($time, 0, irq ? 1 : 0);"));
        // A slave-only IP gets no bridge.
        let m = parse_module(&fixture("math/my_math_ip.v"), None).unwrap();
        let axi = detect_axi_lite(&m).unwrap();
        assert_eq!(detect_axi_master(&m), Ok(None));
        assert!(!generate_testbench(&m, &axi).contains("HP0_DEV = 2"));
    }

    #[test]
    fn an_incomplete_master_interface_is_reported() {
        let src = fixture("dma/dma_xform.v").replace("m_wlast", "m_wlast_x");
        let m = parse_module(&src, None).unwrap();
        let e = detect_axi_master(&m).unwrap_err();
        assert!(e.contains("wlast"), "{e}");
    }

    #[test]
    fn parameter_expressions_evaluate() {
        let mut v = HashMap::new();
        v.insert("W".to_string(), 32);
        assert_eq!(eval("W/8-1", &v), Ok(3));
        assert_eq!(eval("$clog2(1024)", &v), Ok(10));
        assert_eq!(eval("(W+4)*2", &v), Ok(72));
        assert_eq!(eval("32'd16", &v), Ok(16));
        assert_eq!(eval("8'hFF", &v), Ok(255));
        assert!(eval("X+1", &v).unwrap_err().contains("unknown parameter"));
    }

    #[test]
    fn protocol_checkers_are_instantiated_for_both_bridges_and_can_be_compiled_out() {
        // The DMA fixture has an AXI4-Lite slave and an AXI4 master.
        let src = fixture("dma/dma_xform.v");
        let m = parse_module(&src, None).unwrap();
        let axi = detect_axi_lite(&m).unwrap();
        let tb = generate_testbench(&m, &axi);
        assert!(tb.contains("`ifndef ZS_NO_PROTOCOL_CHECKERS"), "{tb}");
        assert!(tb.contains("zs_axi_checker #(.ID(0), .FULL(0)"), "{tb}");
        assert!(tb.contains("zs_axi_checker #(.ID(1), .FULL(1)"), "{tb}");
        assert!(tb.contains(".awaddr(m_awaddr)"), "{tb}");
        assert!(tb.contains(".rlast(m_rlast)"), "{tb}");
        assert!(tb.contains(".awlen(m_awlen)"), "{tb}");
        // The checker guard and the Icarus wrappers (ADR-037) are each closed by an `endif`.
        let opened = tb.matches("`ifdef").count() + tb.matches("`ifndef").count();
        assert_eq!(tb.matches("`endif").count(), opened, "{tb}");

        // A lite-only IP gets the lite checker alone.
        let math = parse_module(&fixture("math/my_math_ip.v"), None).unwrap();
        let axi = detect_axi_lite(&math).unwrap();
        let tb = generate_testbench(&math, &axi);
        assert!(tb.contains("zs_chk_lite"), "{tb}");
        assert!(!tb.contains("zs_chk_mst"), "{tb}");
        assert!(
            tb.contains(".awaddr(S_AXI_AWADDR)") || tb.contains("S_AXI_AWADDR"),
            "{tb}"
        );
    }

    #[test]
    fn the_checker_source_names_every_rule_the_host_knows() {
        let sv = protocol_checker_source();
        assert!(
            sv.starts_with("`timescale"),
            "the checker needs a timescale"
        );
        assert!(sv.contains("module zs_axi_checker"));
        assert!(
            sv.contains("zs_protocol_check"),
            "the DPI function is imported"
        );
        for name in &pl_api::protocol::RULES[1..] {
            assert!(
                sv.contains(name),
                "rule {name} is not documented in the checker"
            );
        }
        for name in pl_api::protocol::CHANNELS {
            assert!(sv.contains(name), "channel {name} is not documented");
        }
    }

    #[test]
    fn master_interface_device_maps_standard_ports() {
        assert_eq!(master_interface_device("S_AXI_GP0"), Some(0));
        assert_eq!(master_interface_device("S_AXI_GP1"), Some(1));
        assert_eq!(master_interface_device("S_AXI_HP0"), Some(2));
        assert_eq!(master_interface_device("S_AXI_HP1"), Some(3));
        assert_eq!(master_interface_device("S_AXI_HP2"), Some(4));
        assert_eq!(master_interface_device("S_AXI_HP3"), Some(5));
        assert_eq!(master_interface_device("S_AXI_ACP"), Some(6));
        assert_eq!(master_interface_device("M_AXI_GP0"), None);
    }

    #[test]
    fn multi_slave_testbench_generates_instances_decoders_and_checkers() {
        let math = parse_module(&fixture("math/my_math_ip.v"), None).unwrap();
        let axi0 = detect_axi_lite(&math).unwrap();
        let axi1 = detect_axi_lite(&math).unwrap();

        let slaves = vec![
            SlaveDesc {
                instance: "u_math0",
                module: &math,
                axi: &axi0,
                base: 0x43C0_0000,
                size: 0x1000,
                irq_bits: &[],
                master: None,
            },
            SlaveDesc {
                instance: "u_math1",
                module: &math,
                axi: &axi1,
                base: 0x43C1_0000,
                size: 0x1000,
                irq_bits: &[],
                master: None,
            },
        ];

        let tb = generate_multi_slave_testbench(&slaves, Some(50_000_000));
        assert!(tb.contains("always #10 aclk = ~aclk;   // 50 MHz, from the block design"));
        assert!(tb.contains("my_math_ip u_math0 ("));
        assert!(tb.contains("my_math_ip u_math1 ("));
        assert!(tb.contains("reg  [3:0] u_math0_S_AXI_AWADDR = 0;"));
        assert!(tb.contains("reg  [3:0] u_math1_S_AXI_AWADDR = 0;"));
        assert!(tb.contains("task lite_write_0"));
        assert!(tb.contains("task lite_write_1"));
        assert!(tb.contains("task lite_read_0"));
        assert!(tb.contains("task lite_read_1"));
        assert!(tb.contains("if (u_addr >= 32'h43C00000 && u_addr <= 32'h43C00FFF) begin"));
        assert!(tb.contains("lite_write_0(u_addr - 32'h43C00000, wd_i, ws_i[3:0], wr_ok);"));
        assert!(tb.contains("else if (u_addr >= 32'h43C10000 && u_addr <= 32'h43C10FFF) begin"));
        assert!(tb.contains("lite_write_1(u_addr - 32'h43C10000, wd_i, ws_i[3:0], wr_ok);"));
        assert!(tb.contains("zs_axi_checker #(.ID(0), .FULL(0)"));
        assert!(tb.contains("zs_axi_checker #(.ID(1), .FULL(0)"));
        assert!(tb.contains("zs_chk_lite_0"));
        assert!(tb.contains("zs_chk_lite_1"));
    }

    #[test]
    fn multi_slave_testbench_with_master_and_irqs() {
        let math = parse_module(&fixture("math/my_math_ip.v"), None).unwrap();
        let axi_m = detect_axi_lite(&math).unwrap();
        let dma = parse_module(&fixture("dma/dma_xform.v"), None).unwrap();
        let axi_d = detect_axi_lite(&dma).unwrap();
        let mst = detect_axi_master(&dma).unwrap().expect("master on dma");

        let slaves = vec![
            SlaveDesc {
                instance: "u_math",
                module: &math,
                axi: &axi_m,
                base: 0x43C0_0000,
                size: 0x1000,
                irq_bits: &[],
                master: None,
            },
            SlaveDesc {
                instance: "u_dma",
                module: &dma,
                axi: &axi_d,
                base: 0x43C2_0000,
                size: 0x10000,
                irq_bits: &[Some(4)],
                master: Some((&mst, 6)), // S_AXI_ACP = device 6
            },
        ];

        let tb = generate_multi_slave_testbench(&slaves, None);
        assert!(tb.contains("always #5 aclk = ~aclk;   // 100 MHz = FCLK_CLK0"));
        assert!(tb.contains("// u_dma.irq -> IRQ_F2P[4]"));
        assert!(tb.contains("rp_irq($time, 4, u_dma_irq ? 1 : 0);"));
        assert!(tb.contains("localparam int u_dma_DEV = 6;"));
        assert!(tb.contains("rp_mem_start($time, u_dma_DEV, 0, u_dma_ra, (u_dma_rl + 1) * 4)"));
        assert!(tb.contains("rp_mem_start($time, u_dma_DEV, 1, u_dma_wa, (u_dma_wl + 1) * 4)"));
        assert!(tb.contains("zs_axi_checker #(.ID(0), .FULL(0)"));
        assert!(tb.contains("zs_axi_checker #(.ID(1), .FULL(0)"));
        assert!(tb.contains("zs_axi_checker #(.ID(2), .FULL(1)"));
        assert!(tb.contains("zs_chk_mst_1"));
    }
}

