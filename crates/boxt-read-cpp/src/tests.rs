use boxt_core::Text;
use boxt_core::fact::{Directive, Fact, GuardChain};
use boxt_core::limits::{LimitName, Limits};
use boxt_core::location::{InputRef, Location, RelPath, Scope};
use boxt_core::message::{MacroTable, Note, Region, RegionReason};
use boxt_core::outcome::{Details, NotCheckedReason};
use boxt_core::read::{Exceeded, FactSink, NotRead, ReadCtx, Reader};
use boxt_core::settings::Encoding;

use super::{CppReader, definitions};

#[derive(Default)]
struct Sink {
    facts: Vec<Fact>,
    regions: Vec<Region>,
}

impl FactSink for Sink {
    fn fact(&mut self, fact: Fact) -> Result<(), Exceeded> {
        fact.validate().expect("every fact validates");
        self.facts.push(fact);
        Ok(())
    }

    fn region(&mut self, region: Region) -> Result<(), Exceeded> {
        region.validate().expect("every region validates");
        self.regions.push(region);
        Ok(())
    }

    fn note(&mut self, _: Note) -> Result<(), Exceeded> {
        Ok(())
    }
}

fn base(name: &str) -> Location {
    Location::file(
        InputRef {
            index: 0,
            given: Text::from("/src"),
        },
        Vec::new(),
        RelPath::try_from(name).unwrap(),
    )
}

fn read_with(
    name: &str,
    bytes: &[u8],
    limits: &Limits,
    encoding: Option<Encoding>,
    table: Option<&MacroTable>,
) -> Result<Sink, NotRead> {
    let ctx = ReadCtx::new(base(name), limits, encoding);
    let mut ctx = match table {
        Some(t) => ctx.with_macros(t),
        None => ctx,
    };
    let mut sink = Sink::default();
    CppReader.read(bytes, &mut ctx, &mut sink)?;
    Ok(sink)
}

fn read(name: &str, src: &str) -> Sink {
    read_with(name, src.as_bytes(), &Limits::default(), None, None).expect("read")
}

type CallView<'a> = (String, Vec<String>, u32, Scope, &'a Vec<GuardChain>);

fn calls(sink: &Sink) -> Vec<CallView<'_>> {
    sink.facts
        .iter()
        .filter_map(|f| match f {
            Fact::Call {
                location,
                callee,
                args,
                guards,
                ..
            } => Some((
                callee.as_str().unwrap().to_string(),
                args.iter()
                    .map(|a| a.as_str().unwrap().to_string())
                    .collect(),
                location.line.unwrap(),
                location.scope.clone(),
                guards,
            )),
            _ => None,
        })
        .collect()
}

fn frames(chain: &GuardChain) -> Vec<(Directive, &str, bool)> {
    chain
        .frames
        .iter()
        .map(|f| (f.directive, f.text.as_str().unwrap(), f.holds_code))
        .collect()
}

fn function(name: &str, signature: &str) -> Scope {
    Scope::Function {
        qualified_name: Text::from(name),
        signature: Text::from(signature),
    }
}

#[test]
fn guarded_call() {
    let sink = read(
        "a.c",
        "void f(){ \n#if USE_DONGLE && !defined(_WIN32)\n setuid(0);\n#endif\n}\n",
    );
    let calls = calls(&sink);
    assert_eq!(calls.len(), 1);
    let (callee, args, line, scope, guards) = &calls[0];
    assert_eq!(callee, "setuid");
    assert_eq!(args, &vec!["0".to_string()]);
    assert_eq!(*line, 3);
    assert_eq!(scope, &function("f", "()"));
    assert_eq!(guards.len(), 1);
    assert_eq!(
        frames(&guards[0]),
        vec![(Directive::If, "#if USE_DONGLE && !defined(_WIN32)", true)]
    );
    assert_eq!(
        guards[0].frames[0].identifiers.as_slice(),
        &[Text::from("USE_DONGLE"), Text::from("_WIN32")]
    );
    assert!(sink.regions.is_empty());
}

#[test]
fn nested_in_else() {
    let sink = read(
        "a.c",
        "void t(void) {\n#if A\n  a();\n#else\n#ifdef B\n  g();\n#endif\n#endif\n}\n",
    );
    let calls = calls(&sink);
    let (_, _, _, _, guards) = calls.iter().find(|c| c.0 == "g").unwrap();
    assert_eq!(guards.len(), 2);
    assert_eq!(
        frames(&guards[0]),
        vec![
            (Directive::If, "#if A", false),
            (Directive::Else, "#else", true)
        ]
    );
    assert_eq!(
        frames(&guards[1]),
        vec![(Directive::Ifdef, "#ifdef B", true)]
    );
    assert!(guards[0].has_alternative);
    assert!(!guards[1].has_alternative);
}

#[test]
fn error_branch_is_no_alternative() {
    let sink = read(
        "a.c",
        "void t(void) {\n#ifdef _WIN32\n  f();\n#else\n#error x\n#endif\n}\n",
    );
    let calls = calls(&sink);
    let guards = calls[0].4;
    assert_eq!(
        frames(&guards[0]),
        vec![(Directive::Ifdef, "#ifdef _WIN32", true)]
    );
    assert!(!guards[0].has_alternative);
}

#[test]
fn includes() {
    let sink = read(
        "a.c",
        "#include <sys/types.h>\n#  include \"x.h\" // c\n#include FOO\n",
    );
    let found: Vec<(&str, bool, u32)> = sink
        .facts
        .iter()
        .filter_map(|f| match f {
            Fact::Include {
                target,
                system,
                location,
                ..
            } => Some((target.as_str().unwrap(), *system, location.line.unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(found, vec![("sys/types.h", true, 1), ("x.h", false, 2)]);
}

#[test]
fn split_statement_is_read_by_passes() {
    let sink = read(
        "a.c",
        "void f(void) {\n#if A\n  g(1,\n#else\n  g(2,\n#endif\n  3);\n}\n",
    );
    assert!(sink.regions.is_empty(), "{:?}", sink.regions);
    let calls = calls(&sink);
    let gs: Vec<_> = calls.iter().filter(|c| c.0 == "g").collect();
    assert_eq!(gs.len(), 2);
    assert_eq!(gs[0].1, vec!["1".to_string(), "3".to_string()]);
    assert_eq!(gs[0].2, 3);
    assert_eq!(frames(&gs[0].4[0]), vec![(Directive::If, "#if A", true)]);
    assert_eq!(gs[1].1, vec!["2".to_string(), "3".to_string()]);
    assert_eq!(
        frames(&gs[1].4[0]),
        vec![
            (Directive::If, "#if A", false),
            (Directive::Else, "#else", true)
        ]
    );
    assert!(gs.iter().all(|g| g.3 == function("f", "(void)")));
}

#[test]
fn unreadable_region_leaves_the_rest() {
    let src = "int ok(int a) {\n  return puts(\"hi\");\n}\n\nstatic char *\nf(a)\n\tchar *a;\n{\n  return a;\n}\n\nint later(void) {\n  return h(2);\n}\n";
    let sink = read("a.c", src);
    assert!(!sink.regions.is_empty());
    for r in &sink.regions {
        assert_eq!(r.reason, RegionReason::Unparseable);
        assert!(r.first_line >= 5 && r.last_line <= 11, "{r:?}");
    }
    let names: Vec<String> = calls(&sink).into_iter().map(|c| c.0).collect();
    assert!(names.contains(&"puts".to_string()));
    assert!(names.contains(&"h".to_string()));
    let hi = sink.facts.iter().any(|f| {
        matches!(f, Fact::StringLiteral { value, location, .. }
            if value == &Text::from("hi") && location.scope == function("ok", "(int)"))
    });
    assert!(hi);
}

#[test]
fn nothing_readable_is_unparseable() {
    let noise = "@@ ``` $$ \x01\x02 ?? @\n``` @@@ \x7f ` @\n";
    let err = read_with("noise.c", noise.as_bytes(), &Limits::default(), None, None)
        .err()
        .expect("not read");
    assert_eq!(err.reason, NotCheckedReason::Unparseable);
}

#[test]
fn comments_only_is_read() {
    let sink = read("a.h", "/* nothing */\n// here\n");
    assert!(sink.facts.is_empty() && sink.regions.is_empty());
    let sink = read("e.c", "");
    assert!(sink.facts.is_empty() && sink.regions.is_empty());
}

#[test]
fn undeclared_latin1_is_not_checked() {
    let err = read_with(
        "a.c",
        b"char *s = \"caf\xe9\";\n",
        &Limits::default(),
        None,
        None,
    )
    .err()
    .unwrap();
    assert_eq!(err.reason, NotCheckedReason::EncodingUndeclared);
    let sink = read_with(
        "a.c",
        b"char *s = \"caf\xe9\";\n",
        &Limits::default(),
        Some(Encoding::Latin1),
        None,
    )
    .unwrap();
    assert!(sink.facts.iter().any(|f| matches!(f,
        Fact::StringLiteral { value, .. } if value == &Text::from("café"))));
    let err = read_with(
        "a.c",
        b"x\xff",
        &Limits::default(),
        Some(Encoding::Utf8),
        None,
    )
    .err()
    .unwrap();
    assert_eq!(err.reason, NotCheckedReason::InvalidForDeclaredEncoding);
}

fn big_source() -> String {
    let mut src = String::new();
    for i in 0..200 {
        src += &format!("int f{i}(int a) {{ return g(a, {i}) + h(\"{i}\"); }}\n");
    }
    src
}

#[test]
fn over_work_is_not_checked_and_stable() {
    let limits = Limits::default().with(LimitName::Work, 20).unwrap();
    let run = || {
        read_with("a.c", big_source().as_bytes(), &limits, None, None)
            .err()
            .expect("over work")
    };
    let first = run();
    assert_eq!(
        first.reason,
        NotCheckedReason::LimitExceeded(LimitName::Work)
    );
    assert_eq!(
        first.details,
        Details::Measured {
            value: 21,
            limit: 20
        }
    );
    assert_eq!(run(), first);
}

#[test]
fn over_syntax_depth_is_not_checked_and_stable() {
    let src = format!("int x = {}1{};\n", "(".repeat(60), ")".repeat(60));
    let limits = Limits::default().with(LimitName::SyntaxDepth, 40).unwrap();
    let run = || {
        read_with("a.c", src.as_bytes(), &limits, None, None)
            .err()
            .expect("too deep")
    };
    let first = run();
    assert_eq!(
        first.reason,
        NotCheckedReason::LimitExceeded(LimitName::SyntaxDepth)
    );
    assert!(matches!(first.details, Details::Measured { value, limit: 40 } if value > 60));
    assert_eq!(run(), first);
    assert!(read_with("a.c", src.as_bytes(), &Limits::default(), None, None).is_ok());
}

#[test]
fn macro_idioms_parse_and_keep_original_text() {
    let src = "NS_BEGIN\nclass EXPORT X {\n  void m() { run(\"a\"); }\n};\nvoid g() {\n  TAILQ_FOREACH(x, &h, e) {\n    use(x);\n  }\n}\nNS_END\n";
    let sink = read("x.cpp", src);
    assert!(sink.regions.is_empty(), "{:?}", sink.regions);
    let calls = calls(&sink);
    let run = calls.iter().find(|c| c.0 == "run").unwrap();
    assert_eq!(run.3, function("X::m", "()"));
    let each = calls.iter().find(|c| c.0 == "TAILQ_FOREACH").unwrap();
    assert_eq!(each.1, vec![String::new(), String::new(), String::new()]);
    assert!(calls.iter().all(|c| c.0 != "while"));
    for fact in &sink.facts {
        let json = serde_json::to_string(fact).unwrap();
        assert!(!json.contains("while"), "{json}");
    }
}

#[test]
fn guard_regions_describe_each_branch() {
    let src = "#if A\nint a;\n#elif B\nint b;\n#else\n#ifdef C\nint c;\n#endif\n#endif\n";
    let sink = read("a.c", src);
    let regions: Vec<(u32, u32, u32, usize)> = sink
        .facts
        .iter()
        .filter_map(|f| match f {
            Fact::GuardRegion {
                location,
                region,
                guards,
                end_line,
                ..
            } => Some((location.line.unwrap(), region.0, *end_line, guards.len())),
            _ => None,
        })
        .collect();
    assert_eq!(
        regions,
        vec![(1, 0, 3, 1), (3, 1, 5, 1), (5, 2, 9, 1), (6, 3, 8, 2)]
    );
}

#[test]
fn function_scope_is_layout_free() {
    let a = read(
        "a.cpp",
        "namespace n {\nint K::f(const char *name /* c */, int\n   count = 3, ...) {\n  return g();\n}\n}\n",
    );
    let b = read(
        "b.cpp",
        "namespace n { int K :: f(const char *x, int c=4, ...) { return g(); } }\n",
    );
    let scope = |s: &Sink| calls(s)[0].3.clone();
    assert_eq!(scope(&a), function("n::K::f", "(const char *, int, ...)"));
    assert_eq!(scope(&a), scope(&b));
}

#[test]
fn crlf_never_reaches_a_value() {
    let sink = read(
        "a.c",
        "int f(int a,\r\n      char *b)\r\n{\r\n  return g(\"x\",\r\n    1);\r\n}\r\n",
    );
    let calls = calls(&sink);
    assert_eq!(calls[0].1, vec!["\"x\"".to_string(), "1".to_string()]);
    assert_eq!(calls[0].3, function("f", "(int, char *)"));
}

#[test]
fn a_cpp_header_named_h_reads_as_cpp() {
    let src = "namespace a {\ntemplate <class T> class B : public C<T> {\n public:\n  void f() { g(1); }\n};\n}\n";
    let sink = read("b.h", src);
    assert!(sink.regions.is_empty(), "{:?}", sink.regions);
    assert_eq!(calls(&sink)[0].3, function("a::B::f", "()"));
}

#[test]
fn macro_expansion_reads_a_damaged_line_with_a_table() {
    let header = "#define Lib_Call\n";
    let defs = definitions(header.as_bytes(), None, &base("d.h")).unwrap();
    let table = MacroTable::from(defs);
    let src = "int Lib_Call handler(int a, int b)\n{\n  return run(a, b);\n}\n";
    let without = read_with("a.c", src.as_bytes(), &Limits::default(), None, None).unwrap();
    let with = read_with(
        "a.c",
        src.as_bytes(),
        &Limits::default(),
        None,
        Some(&table),
    )
    .unwrap();
    assert!(!without.regions.is_empty());
    assert!(with.regions.is_empty(), "{:?}", with.regions);
    assert_eq!(calls(&with)[0].3, function("handler", "(int, int)"));
    let src = "void f(void) {\n  int v = 1;\n  Weird_If(v) use(v Plus_Op 2);\n  done(\"x\");\n}\n";
    let header = "#define Weird_If(x) if (x)\n#define Plus_Op +\n";
    let table = MacroTable::from(definitions(header.as_bytes(), None, &base("d.h")).unwrap());
    let plain = read_with("a.c", src.as_bytes(), &Limits::default(), None, None).unwrap();
    let expanded = read_with(
        "a.c",
        src.as_bytes(),
        &Limits::default(),
        None,
        Some(&table),
    )
    .unwrap();
    let lines = |s: &Sink| {
        s.regions
            .iter()
            .map(|r| r.last_line - r.first_line + 1)
            .sum::<u32>()
    };
    assert!(lines(&plain) > 0, "{:?}", plain.regions);
    assert_eq!(lines(&expanded), 0, "{:?}", expanded.regions);
    let weird = calls(&expanded)
        .into_iter()
        .find(|c| c.0 == "Weird_If")
        .unwrap();
    assert_eq!(weird.2, 3);
    assert_eq!(weird.3, function("f", "(void)"));
    let names: Vec<String> = calls(&expanded).into_iter().map(|c| c.0).collect();
    assert!(names.contains(&"use".to_string()), "{names:?}");
    for fact in &expanded.facts {
        let json = serde_json::to_string(fact).unwrap();
        assert!(!json.contains("if (") && !json.contains("+"), "{json}");
    }
}

#[test]
fn output_is_deterministic() {
    let src = big_source();
    let a = read("a.c", &src);
    let b = read("a.c", &src);
    assert_eq!(a.facts, b.facts);
    assert_eq!(a.regions, b.regions);
}

#[test]
fn a_utf8_mark_and_utf16_files_read() {
    let sink = read("a.c", "\u{FEFF}int f(void) { return g(1); }\n");
    assert_eq!(calls(&sink)[0].0, "g");
    let mut le = vec![0xFF, 0xFE];
    for unit in "int f(void) { return g(2); }\n".encode_utf16() {
        le.extend_from_slice(&unit.to_le_bytes());
    }
    let sink = read_with("a.c", &le, &Limits::default(), None, None).unwrap();
    assert_eq!(calls(&sink)[0].1, vec!["2".to_string()]);
}

#[test]
fn grammar_follows_the_extension() {
    use super::Grammar;
    assert_eq!(Grammar::for_file_name(b"a/b.c"), Some(Grammar::C));
    assert_eq!(Grammar::for_file_name(b"b.H"), Some(Grammar::C));
    assert_eq!(Grammar::for_file_name(b"b.C"), Some(Grammar::Cpp));
    assert_eq!(Grammar::for_file_name(b"b.cpp"), Some(Grammar::Cpp));
    assert_eq!(Grammar::for_file_name(b"b.hpp"), Some(Grammar::Cpp));
    assert_eq!(Grammar::for_file_name(b"b.txt"), None);
    assert_eq!(Grammar::for_file_name(b"Makefile"), None);
    let both: &[Grammar] = &[Grammar::C, Grammar::Cpp];
    assert_eq!(Grammar::tried_for(Some(b"a.c")), &[Grammar::C]);
    assert_eq!(Grammar::tried_for(Some(b"a.h")), both);
    assert_eq!(Grammar::tried_for(Some(b"a.H")), both);
    assert_eq!(Grammar::tried_for(Some(b"a.C")), &[Grammar::Cpp]);
    assert_eq!(Grammar::tried_for(Some(b"a.cpp")), &[Grammar::Cpp]);
    assert_eq!(Grammar::tried_for(Some(b"vector")), both);
    assert_eq!(Grammar::tried_for(None), both);
}

#[test]
fn an_upper_case_c_file_reads_as_cpp() {
    let sink = read("a.C", "namespace n { void f() { g(1); } }\n");
    assert!(sink.regions.is_empty(), "{:?}", sink.regions);
    assert_eq!(calls(&sink)[0].3, function("n::f", "()"));
}

#[test]
fn an_expansion_fan_out_stops_at_the_work_limit() {
    let mut header = String::new();
    for d in 0..12 {
        header += &format!("#define F{d} F{n} F{n} F{n} F{n}\n", n = d + 1);
    }
    header += "#define F12\n";
    let table = MacroTable::from(definitions(header.as_bytes(), None, &base("d.h")).unwrap());
    let src = "void f(void) {\n  int x = F0 @;\n}\n";
    let limits = Limits::default().with(LimitName::Work, 100_000).unwrap();
    let started = std::time::Instant::now();
    let err = read_with("a.c", src.as_bytes(), &limits, None, Some(&table))
        .err()
        .expect("over work");
    assert_eq!(err.reason, NotCheckedReason::LimitExceeded(LimitName::Work));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
}

#[test]
fn one_literal_rule_for_calls_and_invocations() {
    let args = "1, -2, \"a\" \"b\", 'c', true, nullptr, x, NULL, 3_km";
    let src = format!("void f(void) {{\n  g({args});\n}}\nclass A {{\n  DECL_THING({args})\n}};\n");
    let sink = read("a.cpp", &src);
    let calls = calls(&sink);
    let call = calls
        .iter()
        .find(|c| c.0 == "g")
        .unwrap_or_else(|| panic!("{calls:?} {:?}", sink.regions));
    let inv = calls
        .iter()
        .find(|c| c.0 == "DECL_THING")
        .unwrap_or_else(|| panic!("{calls:?} {:?}", sink.regions));
    let want: Vec<String> = [
        "1",
        "-2",
        "\"a\" \"b\"",
        "'c'",
        "true",
        "nullptr",
        "",
        "",
        "",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(call.1, want);
    assert_eq!(inv.1, want);
}

#[test]
fn invocation_arguments_split_on_parentheses_only() {
    let src = "class A {\n  DECL_THING(1, {2, 3}, [4, 5])\n};\n";
    let sink = read("a.cpp", src);
    let inv = calls(&sink)
        .into_iter()
        .find(|c| c.0 == "DECL_THING")
        .unwrap();
    assert_eq!(inv.1, vec!["1", "", "", "", ""]);
}

#[test]
fn callees_are_normalized() {
    let sink = read(
        "a.cpp",
        "void f() {\n  a ::\n   b /* c */ :: g(1);\n  obj . run \\\n (2);\n}\n",
    );
    let names: Vec<String> = calls(&sink).into_iter().map(|c| c.0).collect();
    assert_eq!(names, vec!["a::b::g", "obj . run"]);
}

fn includes_of(sink: &Sink) -> Vec<(&str, bool)> {
    sink.facts
        .iter()
        .filter_map(|f| match f {
            Fact::Include { target, system, .. } => Some((target.as_str().unwrap(), *system)),
            _ => None,
        })
        .collect()
}

#[test]
fn includes_after_comments_and_splices_and_include_next() {
    let sink = read(
        "a.c",
        "#include /* c */ <a.h>\n#include \\\n  \"b.h\"\n#include_next <c.h>\n#include/**/\"d.h\"\n",
    );
    assert_eq!(
        includes_of(&sink),
        vec![("a.h", true), ("b.h", false), ("c.h", true), ("d.h", false)]
    );
}

fn strings_of(sink: &Sink) -> Vec<(&str, u32)> {
    sink.facts
        .iter()
        .filter_map(|f| match f {
            Fact::StringLiteral {
                value, location, ..
            } => Some((value.as_str().unwrap(), location.line.unwrap())),
            _ => None,
        })
        .collect()
}

#[test]
fn directives_and_linkage_names_yield_no_facts() {
    let sink = read(
        "a.cpp",
        "#define LOG(x) printf(\"%s\", x)\n#if FOO(\"q\")\n#endif\nextern \"C\" {\nvoid f() { puts(\"in\"); }\n}\n",
    );
    assert_eq!(strings_of(&sink), vec![("in", 5)]);
    let names: Vec<String> = calls(&sink).into_iter().map(|c| c.0).collect();
    assert_eq!(names, vec!["puts"]);
}

#[test]
fn raw_and_concatenated_strings() {
    let sink = read(
        "a.cpp",
        "void f() {\n  g(R\"x(a\"b)x\");\n  h(\"one\"\n    \"two\" PRIX \"three\");\n}\n",
    );
    assert_eq!(strings_of(&sink), vec![("a\"b", 2), ("onetwothree", 3)]);
}

#[test]
fn a_header_tie_keeps_the_c_reading() {
    let src = "void f(void) {\n  a<b>(c);\n  @@@;\n}\n";
    let as_c = read("t.c", src);
    let as_cpp = read("t.cpp", src);
    let lines = |s: &Sink| {
        s.regions
            .iter()
            .map(|r| r.last_line - r.first_line + 1)
            .sum::<u32>()
    };
    assert_eq!(lines(&as_c), lines(&as_cpp));
    assert!(lines(&as_c) > 0);
    let header = read("t.h", src);
    assert_eq!(header.facts, as_c.facts);
    assert_ne!(as_c.facts, as_cpp.facts);
}

#[test]
fn a_coverage_pass_reads_a_branch_nested_in_else() {
    let src = "void f(void) {\n#if A\n  g(1,\n#else\n#ifdef B\n  h(2);\n#else\n  k(3);\n#endif\n  g(2,\n#endif\n  3);\n}\n";
    let sink = read("a.c", src);
    assert!(sink.regions.is_empty(), "{:?}", sink.regions);
    let calls = calls(&sink);
    let h = calls
        .iter()
        .find(|c| c.0 == "h")
        .expect("h read by a coverage pass");
    assert_eq!(
        frames(&h.4[0]),
        vec![
            (Directive::If, "#if A", false),
            (Directive::Else, "#else", true)
        ]
    );
    assert_eq!(frames(&h.4[1]), vec![(Directive::Ifdef, "#ifdef B", true)]);
}

#[test]
fn the_reader_charges_its_buffers() {
    let src = "void f(void) {\n#if A\n  g(1,\n#else\n  g(2,\n#endif\n  3);\n}\n";
    let limits = Limits::default().with(LimitName::Memory, 64).unwrap();
    let err = read_with("a.c", src.as_bytes(), &limits, None, None)
        .err()
        .expect("over memory");
    assert_eq!(
        err.reason,
        NotCheckedReason::LimitExceeded(LimitName::Memory)
    );
    let big = big_source();
    let limits = Limits::default()
        .with(LimitName::Memory, big.len() as u64 + 1024)
        .unwrap();
    let err = read_with("a.c", big.as_bytes(), &limits, None, None)
        .err()
        .expect("facts over memory");
    assert_eq!(
        err.reason,
        NotCheckedReason::LimitExceeded(LimitName::Memory)
    );
    assert!(read_with("a.c", src.as_bytes(), &Limits::default(), None, None).is_ok());
}
