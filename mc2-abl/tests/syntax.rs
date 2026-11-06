//! Scanner and parser behaviour: accepted syntax, the exact error codes/messages of the C++ compiler, directives
//! and includes, limits.

mod common;

use common::*;
use mc2_abl::*;

fn compile(src: &str) -> Result<(), AblError> {
    let mut env = Env::new(
        Config::default(),
        single("t.abl", src),
        Box::new(DefaultHost),
    );
    let r = env.pre_process("t.abl").map(|_| ());
    dump_syntax_case(src, &r);
    r
}

/// With `ABL_DUMP_SYNTAX=<dir>` every compiled source is written out together with this implementation's outcome so
/// `oracle-tools syntax-diff` can compare it with the C++ compiler.
fn dump_syntax_case(src: &str, r: &Result<(), AblError>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let Some(dir) = std::env::var_os("ABL_DUMP_SYNTAX") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let n = N.fetch_add(1, Ordering::SeqCst);
    let tag = format!("{}_{n}", std::process::id());
    let path = dir.join("data/missions/syn");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join(format!("{tag}.abl")), src).unwrap();
    let outcome = match r {
        Ok(()) => "OK".to_string(),
        Err(e) => e.message.replace(['\n', '\r'], " ").trim_end().to_string(),
    };
    use std::io::Write;
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("syntax_expected.txt"))
        .unwrap();
    f.write_all(format!("{tag}\t{outcome}\n").as_bytes())
        .unwrap();
}

fn code_of(src: &str) -> SyntaxCode {
    match compile(src) {
        Ok(()) => panic!("expected a syntax error for:\n{src}"),
        Err(e) => e
            .syntax_code()
            .unwrap_or_else(|| panic!("not a syntax error: {e}\n{src}")),
    }
}

fn body(vars: &str, code: &str) -> String {
    format!("module t : integer;\nvar\n{vars}\ncode\n{code}\nendmodule.\n")
}

#[test]
fn error_message_format_matches_the_cxx() {
    let e = compile("module t : integer;\ncode\n  x = 1;\nendmodule.\n").unwrap_err();
    assert_eq!(
        e.message,
        "SYNTAX ERROR t.abl [line 3] - (type 13) Undefined identifier \"x\"\n"
    );
    assert_eq!(e.syntax_code(), Some(SyntaxCode::UndefinedIdentifier));
    assert_eq!((e.file.as_str(), e.line), ("t.abl", 3));
}

#[test]
fn accepted_minimal_forms() {
    compile("module t;\ncode\nendmodule.\n").unwrap();
    compile("MODULE T : INTEGER ;\nCODE\nreturn(1);\nENDMODULE.\n").unwrap();
    compile("library lib;\nfunction f;\ncode\nendfunction;\nendlibrary.\n").unwrap_err(); // library header needs the loader
    compile("fsm f;\nstate start;\ncode\nendstate;\nendfsm.\n").unwrap();
}

#[test]
fn statements_need_separators() {
    // The follow-set check (`synchronize(statementEndList)`) fires before the "missing semicolon" test.
    assert_eq!(
        code_of(&body("integer x; integer y;", "x = 1\n y = 2;\nreturn(x);")),
        SyntaxCode::UnexpectedToken
    );
    // endmodule is not in the statement-end set, so the last statement needs its semicolon.
    assert_eq!(
        code_of("module t : integer;\ncode\nreturn(1)\nendmodule.\n"),
        SyntaxCode::UnexpectedToken
    );
}

#[test]
fn identifier_errors() {
    assert_eq!(
        code_of(&body("integer x; integer x;", "return(1);")),
        SyntaxCode::RedefinedIdentifier
    );
    assert_eq!(
        code_of(&body("", "return(nope);")),
        SyntaxCode::UndefinedIdentifier
    );
    assert_eq!(
        code_of(&body("", "nope = 3;")),
        SyntaxCode::UndefinedIdentifier
    );
    assert_eq!(
        code_of("module t : integer;\nvar\nfoo bar;\ncode\nendmodule.\n"),
        SyntaxCode::UndefinedIdentifier
    );
}

#[test]
fn type_checking_follows_identity_rules() {
    assert_eq!(
        code_of(&body("integer i; real r;", "i = r;")),
        SyntaxCode::IncompatibleAssignment
    );
    compile(&body("integer i; real r;", "r = i; return(1);")).unwrap();
    assert_eq!(
        code_of(&body("integer i; boolean b;", "b = i;")),
        SyntaxCode::IncompatibleAssignment
    );
    // Arrays are only assignable when they share the same type object (or are char arrays that fit).
    assert_eq!(
        code_of(&body("real[3] a; real[3] b;", "a = b;")),
        SyntaxCode::IncompatibleAssignment
    );
    compile("module t : integer;\ntype\n P = real[3];\nvar\n P a; P b;\ncode\n a = b;\n return(1);\nendmodule.\n").unwrap();
    compile(&body("char[8] a; char[4] b;", "a = b; return(1);")).unwrap();
    assert_eq!(
        code_of(&body("char[4] a; char[8] b;", "a = b;")),
        SyntaxCode::IncompatibleAssignment
    );
    // Operators.
    assert_eq!(
        code_of(&body("boolean b; integer i;", "b = b and i;")),
        SyntaxCode::IncompatibleTypes
    );
    // `div` is not a reserved word, so it is just an identifier following the expression
    assert_eq!(
        code_of(&body("integer i;", "i = 3 div 2;")),
        SyntaxCode::UnexpectedToken
    );
    assert_eq!(
        code_of(&body("real r;", "r = 1.5 mod 2;")),
        SyntaxCode::IncompatibleTypes
    );
    assert_eq!(
        code_of(&body("integer i;", "if (i) then endif;")),
        SyntaxCode::IncompatibleTypes
    );
    assert_eq!(
        code_of(&body("integer i;", "while (i + 1) do endwhile;")),
        SyntaxCode::IncompatibleTypes
    );
    assert_eq!(
        code_of(&body("integer i; boolean b;", "if (i == b) then endif;")),
        SyntaxCode::IncompatibleTypes
    );
    assert_eq!(
        code_of(&body("boolean b;", "b = - true;")),
        SyntaxCode::IncompatibleTypes
    );
    assert_eq!(
        code_of(&body("integer[3] a; real r;", "a[r] = 1;")),
        SyntaxCode::IncompatibleTypes
    );
    assert_eq!(
        code_of(&body("integer a;", "a[1] = 1;")),
        SyntaxCode::TooManySubscripts
    );
    // return() must match the routine's type exactly.
    assert_eq!(
        code_of(&body("real r;", "return(r);")),
        SyntaxCode::IncompatibleTypes
    );
    assert_eq!(
        code_of("module t : integer;\ncode\nreturn;\nendmodule.\n"),
        SyntaxCode::WrongNumberOfParams
    );
}

#[test]
fn routine_declarations() {
    let funcs = "function f(integer a, @real b);\ncode\nendfunction;\n";
    let mk = |code: &str| {
        format!("module t : integer;\nvar\n real r; integer i;\n{funcs}\ncode\n{code}\nreturn(0);\nendmodule.\n")
    };
    compile(&mk("f(1, r);")).unwrap();
    assert_eq!(code_of(&mk("f(1);")), SyntaxCode::WrongNumberOfParams);
    assert_eq!(code_of(&mk("f(1, r, 3);")), SyntaxCode::WrongNumberOfParams);
    assert_eq!(code_of(&mk("f(1, 2.5);")), SyntaxCode::InvalidRefParam);
    assert_eq!(code_of(&mk("f(1, i);")), SyntaxCode::IncompatibleTypes);
    assert_eq!(code_of(&mk("f(1.5, r);")), SyntaxCode::IncompatibleTypes);
    // order functions must return integer
    assert_eq!(
        code_of("module t;\norder o;\ncode\nendorder;\ncode\nendmodule.\n"),
        SyntaxCode::OrderReturnType
    );
    // functions cannot nest
    assert_eq!(
        code_of("module t;\nfunction a;\nfunction b;\ncode\nendfunction;\ncode\nendfunction;\ncode\nendmodule.\n"),
        SyntaxCode::NoFunctionNesting
    );
    // redefinition of a parameterless function
    assert_eq!(
        code_of("module t;\nfunction a;\ncode\nendfunction;\nfunction a;\ncode\nendfunction;\ncode\nendmodule.\n"),
        SyntaxCode::RedefinedIdentifier
    );
    // void functions cannot be used in expressions
    assert_eq!(
        code_of(
            "module t : integer;\nfunction a;\ncode\nendfunction;\ncode\nreturn(a);\nendmodule.\n"
        ),
        SyntaxCode::IncompatibleTypes
    );
    assert_eq!(
        code_of(
            "module t;\nfunction a(integer x, integer x);\ncode\nendfunction;\ncode\nendmodule.\n"
        ),
        SyntaxCode::RedefinedIdentifier
    );
}

#[test]
fn forward_declared_functions_and_states() {
    compile("module t;\nfunction a;\nforward;\nfunction b;\ncode\n a;\nendfunction;\nfunction a;\ncode\nendfunction;\ncode\nendmodule.\n").unwrap();
    // a state may be targeted before it is defined
    compile("fsm f;\nstate start;\ncode\n trans later;\nendstate;\nstate later;\ncode\nendstate;\nendfsm.\n").unwrap();
    // `endstate` is in the statement-end set, so a final `trans x` may omit its semicolon - but the C++ then looks up
    // the word *after* the identifier (`endstate`) and creates a dangling forward state for it.
    let mut env = Env::new(
        Config::default(),
        single(
            "t.abl",
            "fsm f;\nstate start;\ncode\n trans start\nendstate;\nendfsm.\n",
        ),
        Box::new(DefaultHost),
    );
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    let e = env.execute(inst, &mut NullBindings).unwrap_err();
    assert!(
        e.message.contains("Undefined ABL RoutineKey"),
        "{}",
        e.message
    );
    // trans needs a state name
    assert_eq!(
        code_of("fsm f;\nstate start;\ncode\n trans 3;\nendstate;\nendfsm.\n"),
        SyntaxCode::MissingStateIdentifier
    );
}

#[test]
fn statement_structure_errors() {
    assert_eq!(
        code_of(&body("integer i;", "if (true) then i = 1;")),
        SyntaxCode::MissingEndIf
    );
    assert_eq!(
        code_of(&body("integer i;", "if (true) i = 1; endif;")),
        SyntaxCode::MissingThen
    );
    assert_eq!(
        code_of(&body("integer i;", "while (true) i = 1; endwhile;")),
        SyntaxCode::MissingDo
    );
    assert_eq!(
        code_of(&body("integer i;", "while (true) do i = 1;")),
        SyntaxCode::MissingEndWhile
    );
    assert_eq!(
        code_of(&body("integer i;", "for i = 1 to 3 do i = 2;")),
        SyntaxCode::MissingEndFor
    );
    assert_eq!(
        code_of(&body("integer i;", "for i = 1 3 do endfor;")),
        SyntaxCode::MissingTo
    );
    assert_eq!(
        code_of(&body("integer i;", "for 1 = 1 to 3 do endfor;")),
        SyntaxCode::MissingIdentifier
    );
    assert_eq!(
        code_of(&body("real r;", "for r = 1 to 3 do endfor;")),
        SyntaxCode::IncompatibleTypes
    );
    assert_eq!(
        code_of(&body("integer i;", "repeat i = 1; i = 2;")),
        SyntaxCode::MissingUntil
    );
    assert_eq!(
        code_of(&body("integer i;", "switch i case 1: i = 1; endcase; ")),
        SyntaxCode::MissingEndSwitch
    );
    assert_eq!(
        code_of(&body("integer i;", "switch i case 1: i = 1; ")),
        SyntaxCode::MissingEndCase
    );
    assert_eq!(
        code_of(&body(
            "integer i; integer j;",
            "switch i case j: i = 1; endcase; endswitch;"
        )),
        SyntaxCode::NotAConstantIdentifier
    );
    assert_eq!(
        code_of(&body(
            "real r;",
            "switch r case 1: r = 1.0; endcase; endswitch;"
        )),
        SyntaxCode::IncompatibleTypes
    );
    assert_eq!(
        code_of(&body(
            "integer i;",
            "switch i case 1.5: i = 1; endcase; endswitch;"
        )),
        SyntaxCode::InvalidConstant
    );
    // `elsif` has a token code but is missing from the reserved word tables, so it is an ordinary identifier
    assert_eq!(
        code_of(&body(
            "integer i;",
            "if (true) then i = 1; elsif (false) then i = 2; endif;"
        )),
        SyntaxCode::UndefinedIdentifier
    );
    // missing end of module / period
    assert_eq!(code_of("module t;\ncode\n"), SyntaxCode::UnexpectedEof);
    assert_eq!(
        code_of("module t;\ncode\nreturn;\nendfunction;\n"),
        SyntaxCode::UnexpectedToken
    );
    assert_eq!(
        code_of("module t;\ncode\nendmodule\n"),
        SyntaxCode::MissingPeriod
    );
    assert_eq!(
        code_of("module t;\ncode\nendmodule. foo\n"),
        SyntaxCode::UnexpectedToken
    );
    assert_eq!(code_of("module t;\nvar\n"), SyntaxCode::MissingCode);
    assert_eq!(
        code_of("module ;\ncode\nendmodule.\n"),
        SyntaxCode::MissingIdentifier
    );
    assert_eq!(code_of("modul t;\n"), SyntaxCode::MissingModule);
}

#[test]
fn numbers_and_literals() {
    // The C++ scanner never skips past the 'E', so exponents are always errors; "1." has no fraction digits.
    assert_eq!(
        code_of(&body("real r;", "r = 1e5;")),
        SyntaxCode::InvalidExponent
    );
    assert_eq!(
        code_of(&body("real r;", "r = 1.5e5;")),
        SyntaxCode::InvalidExponent
    );
    assert_eq!(
        code_of(&body("real r;", "r = 1.;")),
        SyntaxCode::InvalidFraction
    );
    assert_eq!(
        code_of(&body("real r;", "r = 123456789012345678901234;")),
        SyntaxCode::TooManyDigits
    );
    compile(&body("real r;", "r = 123456789012345678;")).unwrap();
    // identifiers cannot start with an underscore (the character is an error token)
    assert_eq!(code_of(&body("", "_x = 1;")), SyntaxCode::UnexpectedToken);
    // single quotes do not exist: characters are one-character strings
    assert_eq!(
        code_of(&body("char c;", "c = 'a';")),
        SyntaxCode::InvalidExpression
    );
}

#[test]
fn directives_and_comments() {
    compile(&body("", "// comment\nreturn(1); /* c */")).unwrap();
    assert_eq!(
        code_of(&body("", "/* never closed\nreturn(1);")),
        SyntaxCode::UnexpectedEof
    );
    assert_eq!(
        code_of(&body("", "#bogus\nreturn(1);")),
        SyntaxCode::UnknownLanguageDirective
    );
    assert_eq!(
        code_of(&body("", "#include 5\nreturn(1);")),
        SyntaxCode::BadLanguageDirectiveUsage
    );
    // nested debug blocks are skipped as a unit
    let (env, inst) = run_module(&body(
        "",
        "#debug_start\n#debug_start\nreturn(7);\n#debug_end\nreturn(8);\n#debug_end\nreturn(1);",
    ))
    .unwrap();
    assert_eq!(env.get_integer(inst), 1);
    // comment markers inside strings are still comments (the scanner works on characters): the string runs on to the
    // next quote character on a later line
    let (env, inst) =
        run_module(&body("char[16] s;", "s = \"a//b\ncomment\";\nreturn(5);")).unwrap();
    assert_eq!(env.get_integer(inst), 5);
}

#[test]
fn include_directives() {
    let mut m = MemorySource::new();
    m.insert("data/missions/main.abl", "module main : integer;\nconst\n #include_ \"Consts.ABI\"\nvar\n integer x;\ncode\n #include_ \"body.abi\"\n return(x + Answer);\nendmodule.\n");
    m.insert("data/missions/consts.abi", "Answer = 40;\n");
    m.insert(
        "data/missions/body.abi",
        "x = 2;\n#include_ \"deeper.abi\"\n",
    );
    m.insert("data/missions/deeper.abi", "x = x + 0;\n");
    let mut env = Env::new(Config::default(), m.clone(), Box::new(DefaultHost));
    let h = env.pre_process("data/missions/main.abl").unwrap();
    assert_eq!(env.module(h).source_files.len(), 4);
    let inst = env.new_instance(h).unwrap();
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(inst), 42);

    // `#include` is relative to the working directory instead of the module's directory
    let mut m2 = MemorySource::new();
    m2.insert(
        "data/missions/main.abl",
        "module main : integer;\ncode\n #include \"data/other/inc.abi\"\n return(3);\nendmodule.\n",
    );
    m2.insert("data/other/inc.abi", "return(9);\n");
    let mut env = Env::new(Config::default(), m2, Box::new(DefaultHost));
    let h = env.pre_process("data/missions/main.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(inst), 9);

    // missing includes are fatal
    let mut env = Env::new(Config::default(), m, Box::new(DefaultHost));
    let e = env.pre_process("data/missions/nothere.abl").unwrap_err();
    assert!(
        e.message.contains("unable to open ABL File"),
        "{}",
        e.message
    );
}

#[test]
fn error_in_included_file_reports_that_file_and_line() {
    let mut m = MemorySource::new();
    m.insert(
        "data/m/a.abl",
        "module a;\ncode\n #include_ \"b.abi\"\nendmodule.\n",
    );
    m.insert("data/m/b.abi", "\n\nnope = 1;\n");
    let mut env = Env::new(Config::default(), m, Box::new(DefaultHost));
    let e = env.pre_process("data/m/a.abl").unwrap_err();
    assert_eq!(
        (e.file.as_str(), e.line),
        ("data/m/b.abi", 3),
        "{}",
        e.message
    );
}

#[test]
fn limits_are_configurable() {
    // static variable declarations
    let src = "module t;\nvar\n static integer a;\n static integer b;\n static integer c;\ncode\nendmodule.\n";
    let cfg = Config {
        max_static_variables: 2,
        ..Config::default()
    };
    let mut env = Env::new(cfg, single("t.abl", src), Box::new(DefaultHost));
    assert_eq!(
        env.pre_process("t.abl").unwrap_err().syntax_code(),
        Some(SyntaxCode::TooManyStaticVars)
    );
    // same source is fine with the default limit of 100
    compile(src).unwrap();

    // registered modules
    let mut m = MemorySource::new();
    for i in 0..3 {
        m.insert(
            &format!("m{i}.abl"),
            format!("module m{i};\ncode\nendmodule.\n"),
        );
    }
    let cfg = Config {
        max_registered_modules: 2,
        ..Config::default()
    };
    let mut env = Env::new(cfg, m, Box::new(DefaultHost));
    env.pre_process("m0.abl").unwrap();
    env.pre_process("m1.abl").unwrap();
    assert!(env
        .pre_process("m2.abl")
        .unwrap_err()
        .message
        .contains("too many registered modules"));

    // code segment size
    let cfg = Config {
        max_code_buffer_size: 400,
        ..Config::default()
    };
    let long: String = (0..200).map(|_| "x = x + 1;\n").collect();
    let mut env = Env::new(
        cfg,
        single("t.abl", &body("integer x;", &long)),
        Box::new(DefaultHost),
    );
    assert_eq!(
        env.pre_process("t.abl").unwrap_err().syntax_code(),
        Some(SyntaxCode::CodeSegmentOverflow)
    );

    // too many states in an fsm (nine fit)
    let states = |n: usize| {
        let mut s = String::from("fsm f;\n");
        for i in 0..n {
            s.push_str(&format!(
                "state {};\ncode\nendstate;\n",
                if i == 0 {
                    "start".to_string()
                } else {
                    format!("s{i}")
                }
            ));
        }
        s.push_str("endfsm.\n");
        s
    };
    compile(&states(9)).unwrap();
    assert!(compile(&states(10))
        .unwrap_err()
        .message
        .contains("too many states"));
}

#[test]
fn module_names_and_eternals_are_global_to_the_environment() {
    let mut m = MemorySource::new();
    m.insert(
        "a.abl",
        "module a;\nvar\n eternal integer shared;\ncode\n shared = 5;\nendmodule.\n",
    );
    m.insert(
        "b.abl",
        "module b : integer;\ncode\n return(shared);\nendmodule.\n",
    );
    m.insert(
        "c.abl",
        "module c;\nvar\n eternal integer shared;\ncode\nendmodule.\n",
    );
    m.insert("a2.abl", "module a;\ncode\nendmodule.\n");
    let mut env = Env::new(Config::default(), m, Box::new(DefaultHost));
    let a = env.pre_process("a.abl").unwrap();
    let ia = env.new_instance(a).unwrap();
    env.execute(ia, &mut NullBindings).unwrap();
    let b = env.pre_process("b.abl").unwrap();
    let ib = env.new_instance(b).unwrap();
    env.execute(ib, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(ib), 5);
    assert_eq!(env.eternal_integer("shared"), Some(5));
    assert_eq!(
        env.pre_process("c.abl").unwrap_err().syntax_code(),
        Some(SyntaxCode::RedefinedIdentifier)
    );
    assert_eq!(
        env.pre_process("a2.abl").unwrap_err().syntax_code(),
        Some(SyntaxCode::RedefinedIdentifier)
    );
    // compiling the same file again returns the existing handle
    assert_eq!(env.pre_process("A.ABL").unwrap(), a);
}

#[test]
fn local_variable_size_limit() {
    // 32001 bytes of locals (8001 integers) is "Too Many Local Variables"
    let ok = body("integer[8000] a;", "return(1);");
    compile(&ok).unwrap();
    assert_eq!(
        code_of(&body("integer[8001] a;", "return(1);")),
        SyntaxCode::TooManyLocalVariables
    );
}
