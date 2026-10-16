//! Lexer and preprocessor tests: literals, operators, comments, positions,
//! macros, conditionals, includes and the directives that are recorded.

use rtl_frontend::lexer::{Number, Tok, lex};
use rtl_frontend::{Options, parse_text};
use std::path::PathBuf;

fn toks(src: &str) -> Vec<Tok> {
    lex("t.v", src)
        .into_iter()
        .map(|t| t.tok)
        .filter(|t| !matches!(t, Tok::Eof))
        .collect()
}

fn num(size: Option<u32>, signed: bool, base: char, digits: &str) -> Tok {
    Tok::Num(Number {
        size,
        signed,
        base,
        digits: digits.to_string(),
        apostrophe: base != 'd' || size.is_some(),
    })
}

fn diags_of(src: &str, opts: &Options) -> Vec<String> {
    parse_text("t.sv", src, opts)
        .diagnostics
        .iter()
        .map(|d| format!("{}:{}:{}: {}", d.line, d.column, d.construct, d.message))
        .collect()
}

#[test]
fn identifiers_escaped_identifiers_and_system_names() {
    assert_eq!(
        toks("foo _bar$1"),
        vec![Tok::Ident("foo".into()), Tok::Ident("_bar$1".into())]
    );
    assert_eq!(
        toks("\\bus[3].x  y"),
        vec![Tok::Ident("\\bus[3].x".into()), Tok::Ident("y".into())]
    );
    assert_eq!(toks("$display"), vec![Tok::SysName("$display".into())]);
    assert!(
        matches!(toks("\\ ")[0], Tok::Error(_)),
        "an empty escaped identifier is an error"
    );
}

#[test]
fn sized_based_and_unsized_numbers() {
    assert_eq!(toks("8'hFF"), vec![num(Some(8), false, 'h', "ff")]);
    assert_eq!(toks("4'b10x1"), vec![num(Some(4), false, 'b', "10x1")]);
    assert_eq!(toks("16'sd5"), vec![num(Some(16), true, 'd', "5")]);
    assert_eq!(toks("32'h_DE_AD"), vec![num(Some(32), false, 'h', "dead")]);
    assert_eq!(toks("'hff"), vec![num(None, false, 'h', "ff")]);
    assert_eq!(toks("'0"), vec![num(None, false, '\'', "0")]);
    assert_eq!(toks("'1"), vec![num(None, false, '\'', "1")]);
    assert_eq!(toks("1_000"), vec![num(None, false, 'd', "1000")]);
    assert_eq!(toks("42"), vec![num(None, false, 'd', "42")]);
}

#[test]
fn reals_are_their_own_token() {
    assert_eq!(toks("1.5"), vec![Tok::Real("1.5".into())]);
    assert_eq!(toks("2e3"), vec![Tok::Real("2e3".into())]);
    assert_eq!(toks("1.0e-9"), vec![Tok::Real("1.0e-9".into())]);
}

#[test]
fn invalid_based_digits_are_an_error_token() {
    assert!(matches!(toks("4'b102")[0], Tok::Error(_)));
}

#[test]
fn strings_with_escapes_and_unterminated_strings() {
    assert_eq!(toks(r#""a\nb\"c""#), vec![Tok::Str("a\nb\"c".into())]);
    assert!(matches!(toks("\"abc\n")[0], Tok::Error(_)));
}

#[test]
fn every_operator_longest_match() {
    let ops = [
        "**", "<<<", ">>>", "===", "!==", "+:", "-:", "++", "--", "<<=", ">>=", "~^", "^~", "~&",
        "~|", "&&", "||", "==", "!=", "<=", ">=", "<<", ">>", "::", "->", "+=", "-=", "@", "#",
        "'", "?",
    ];
    for op in ops {
        assert_eq!(toks(op), vec![Tok::Op(op_static(op))], "operator {op}");
    }
    assert_eq!(toks("a<<<=b")[1], Tok::Op("<<<="));
}

fn op_static(s: &str) -> &'static str {
    const ALL: &[&str] = &[
        "**", "<<<", ">>>", "===", "!==", "+:", "-:", "++", "--", "<<=", ">>=", "~^", "^~", "~&",
        "~|", "&&", "||", "==", "!=", "<=", ">=", "<<", ">>", "::", "->", "+=", "-=", "@", "#",
        "'", "?",
    ];
    ALL.iter()
        .find(|x| **x == s)
        .copied()
        .expect("known operator")
}

#[test]
fn comments_are_skipped_and_track_lines() {
    let t = lex("t.v", "a // x\n/* b\n c */ d");
    let kinds: Vec<Tok> = t.iter().map(|t| t.tok.clone()).collect();
    assert_eq!(kinds[0], Tok::Ident("a".into()));
    assert_eq!(kinds[1], Tok::Ident("d".into()));
    assert_eq!(
        t[1].pos.line, 3,
        "line count runs through the block comment"
    );
    assert!(t[1].newline_before);
}

#[test]
fn unterminated_block_comment_is_reported() {
    assert!(matches!(toks("a /* never closed")[1], Tok::Error(_)));
}

#[test]
fn continuation_is_not_a_line_break() {
    let t = lex("t.v", "a \\\n b");
    assert!(
        !t[1].newline_before,
        "a backslash-newline does not end a macro body"
    );
    assert_eq!(t[1].pos.line, 2);
}

#[test]
fn positions_are_one_based_columns() {
    let t = lex("t.v", "  wire x;");
    assert_eq!((t[0].pos.line, t[0].pos.col), (1, 3));
    assert_eq!((t[1].pos.line, t[1].pos.col), (1, 8));
}

#[test]
fn directives_are_tokens_without_the_backtick() {
    assert_eq!(
        toks("`define X"),
        vec![Tok::Directive("define".into()), Tok::Ident("X".into())]
    );
    assert_eq!(toks("`WIDTH"), vec![Tok::Directive("WIDTH".into())]);
}

#[test]
fn unexpected_characters_are_error_tokens_and_do_not_stop_lexing() {
    let t = toks("a ? \u{e9} b");
    assert!(t.iter().any(|x| matches!(x, Tok::Error(_))));
    assert_eq!(t.last(), Some(&Tok::Ident("b".into())));
}

#[test]
fn object_macro_expands_at_use_site() {
    let p = parse_text(
        "t.v",
        "`define W 8\nmodule m; wire [`W-1:0] x; endmodule\n",
        &Options::default(),
    );
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
}

#[test]
fn function_macro_with_arguments_and_defaults() {
    let src = "`define ADD(a, b = 2) (a + b)\n\
               module m; wire x = `ADD(1); wire y = `ADD(1, 3); wire z = `ADD(1, ); endmodule\n";
    let p = parse_text("t.v", src, &Options::default());
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
}

#[test]
fn undefined_macro_and_wrong_argument_count_are_reported() {
    let d = diags_of("module m; wire x = `NOPE; endmodule\n", &Options::default());
    assert!(
        d.iter().any(|x| x.contains("macro: undefined macro `NOPE")),
        "{d:?}"
    );

    let d = diags_of(
        "`define F(a) a\nmodule m; wire x = `F(1, 2); endmodule\n",
        &Options::default(),
    );
    assert!(
        d.iter().any(|x| x.contains("takes 1 arguments, got 2")),
        "{d:?}"
    );
}

#[test]
fn recursive_macro_is_stopped_with_a_diagnostic() {
    let d = diags_of(
        "`define R `R\nmodule m; wire x = `R; endmodule\n",
        &Options::default(),
    );
    assert!(d.iter().any(|x| x.contains("too deep")), "{d:?}");
}

#[test]
fn conditionals_nest_and_select_branches() {
    let src = "`define A\n\
               `ifdef A\n`ifndef B\nmodule m1; endmodule\n`else\nmodule m2; endmodule\n`endif\n\
               `ifdef NOPE\nmodule m3; endmodule\n`elsif A\nmodule m4; endmodule\n`else\nmodule m5; endmodule\n`endif\n";
    let p = parse_text("t.v", src, &Options::default());
    let names: Vec<String> = p.units[0]
        .items
        .iter()
        .filter_map(|i| match i {
            rtl_frontend::ast::Item::Module(m) => Some(m.name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(names, vec!["m1", "m4"]);
}

#[test]
fn unmatched_endif_and_unclosed_ifdef_are_reported() {
    let d = diags_of("`endif\nmodule m; endmodule\n", &Options::default());
    assert!(d.iter().any(|x| x.contains("conditional")), "{d:?}");
    let d = diags_of("`ifdef X\nmodule m; endmodule\n", &Options::default());
    assert!(
        d.iter().any(|x| x.contains("without a matching `endif")),
        "{d:?}"
    );
}

#[test]
fn cli_defines_are_visible_to_ifdef_and_as_values() {
    let opts = Options {
        defines: vec!["WIDTH=12".into(), "FLAG".into()],
        ..Options::default()
    };
    let src = "`ifdef FLAG\nmodule m; wire [`WIDTH-1:0] x; endmodule\n`endif\n";
    let p = parse_text("t.v", src, &opts);
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    assert_eq!(p.units[0].items.len(), 1);
}

#[test]
fn timescale_and_default_nettype_are_recorded() {
    let p = parse_text(
        "t.v",
        "`timescale 1ns/1ps\n`default_nettype none\nmodule m; endmodule\n",
        &Options::default(),
    );
    assert!(p.diagnostics.is_empty(), "{:?}", p.diagnostics);
    assert_eq!(p.units[0].timescale.as_deref(), Some("1ns/1ps"));
    assert_eq!(p.units[0].default_nettype.as_deref(), Some("none"));
}

#[test]
fn include_is_resolved_relative_to_the_file_and_recorded_positions_point_into_it() {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("rtl-frontend-inc-{}", std::process::id()));
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("defs.vh"), "`define N 4\n`define BAD(\n").unwrap();
    std::fs::write(
        dir.join("top.sv"),
        "`include \"sub/defs.vh\"\nmodule m; wire [`N-1:0] x; endmodule\n",
    )
    .unwrap();
    let text = std::fs::read_to_string(dir.join("top.sv")).unwrap();
    let opts = Options {
        include_dirs: vec![],
        ..Options::default()
    };
    let p = parse_text(&dir.join("top.sv").to_string_lossy(), &text, &opts);
    assert!(
        p.diagnostics
            .iter()
            .any(|d| d.construct == "macro" && d.file.ends_with("defs.vh")),
        "the malformed define is reported in the included file: {:?}",
        p.diagnostics
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_include_is_reported_with_its_name() {
    let d = diags_of(
        "`include \"nope_here.vh\"\nmodule m; endmodule\n",
        &Options::default(),
    );
    assert!(
        d.iter()
            .any(|x| x.contains("include: cannot find included file `nope_here.vh`")),
        "{d:?}"
    );
}

#[test]
fn recursive_include_is_rejected() {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("rtl-frontend-rec-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.vh");
    std::fs::write(&a, "`include \"a.vh\"\n").unwrap();
    let text = std::fs::read_to_string(&a).unwrap();
    let p = parse_text(&a.to_string_lossy(), &text, &Options::default());
    assert!(
        p.diagnostics
            .iter()
            .any(|d| d.message.contains("recursive include")),
        "{:?}",
        p.diagnostics
    );
    let _ = std::fs::remove_dir_all(&dir);
}
