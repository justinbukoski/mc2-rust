//! Robustness: randomly damaged programs must produce errors, never panics or hangs.

mod common;

use std::panic::{catch_unwind, AssertUnwindSafe};

use common::*;
use mc2_abl::*;

const BASES: &[&str] = &[
    "module t : integer;\nconst\n K = 3;\ntype\n IL = integer[4];\n Color = (Red, Green, Blue);\nvar\n IL a;\n Color c;\n real r;\n char[12] s;\n integer i;\n static integer st;\nfunction f(integer n, @integer out) : integer;\nvar\n integer j;\ncode\n for j = 1 to n do\n  out = out + j;\n endfor;\n return(out);\nendfunction;\ncode\n a[1] = K;\n c = Green;\n r = 2.5 * a[1];\n s = \"abc\";\n concat(s, 12);\n i = 0;\n while (i < 5) do\n  i = i + 1;\n  if (i == 3) then\n   st = st + f(i, a[2]);\n  else\n   st = st - 1;\n  endif;\n endwhile;\n switch c\n  case Red: i = 1; endcase;\n  case Green, Blue: i = 2; endcase;\n endswitch;\n repeat i = i + 1; until (i > 8);\n return(trunc(r) + i + st + round(r));\nendmodule.\n",
    "fsm t;\nvar\n static integer n;\n static integer m;\norder step : integer;\ncode\n n = n + 1;\n if (n < 3) then\n  return(0);\n endif;\n return(1);\nendorder;\nstate start;\ncode\n step;\n m = m + 1;\n if (m > 2) then\n  trans second;\n endif;\nendstate;\nstate second;\ncode\n transback;\nendstate;\nendfsm.\n",
    "module t : integer;\nvar\n integer[5] list;\n real[3] pos;\n integer n;\ncode\n getObjectPosition(3, pos);\n n = getContacts(list, 1, 2);\n corePower(true);\n setGlobalValue(1, 2.0);\n return(n + trunc(abs(0.0 - pos[1])) + random(5) + getId);\nendmodule.\n",
];

const FRAGMENTS: &[&str] = &[
    "endif;",
    "endwhile;",
    "(",
    ")",
    "[",
    "]",
    ";",
    ",",
    "=",
    "==",
    "<>",
    "var",
    "code",
    "function",
    "state",
    "order",
    "trans",
    "switch",
    "case",
    "endcase;",
    "endswitch;",
    "static",
    "eternal",
    "@",
    "\"",
    "#print_on",
    "#include_ \"x\"",
    "/*",
    "//",
    "1.",
    "9999999999",
    "and",
    "or",
    "not",
    "mod",
    "return(",
    "forward;",
    "integer",
    "real",
    "(0-1)",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const SWAPS: &[(&str, &str)] = &[
    (" + ", " - "),
    (" * ", " / "),
    (" < ", " > "),
    (" == ", " <> "),
    (" and ", " or "),
    ("true", "false"),
    ("trans second", "trans start"),
    ("transback", "trans start"),
    ("return(0)", "return(1)"),
    (" 1;", " 0;"),
    (" 3", " 40"),
    ("endwhile;", "endwhile; endwhile;"),
];

/// Structure-preserving mutations most of the time (so the executor sees many compiled programs), destructive ones
/// otherwise.
fn mutate(rng: &mut Rng, base: &str) -> String {
    let mut text = base.to_string();
    for _ in 0..1 + rng.below(3) {
        match rng.below(10) {
            0..=2 => {
                let (a, b) = SWAPS[rng.below(SWAPS.len())];
                text = text.replacen(a, b, 1);
            }
            3..=4 => {
                let mut lines: Vec<&str> = text.lines().collect();
                if lines.len() > 3 {
                    let i = 1 + rng.below(lines.len() - 2);
                    lines.remove(i);
                }
                text = lines.join("\n") + "\n";
            }
            5 => {
                let mut lines: Vec<&str> = text.lines().collect();
                if lines.len() > 3 {
                    let i = 1 + rng.below(lines.len() - 2);
                    let l = lines[i];
                    lines.insert(i, l);
                }
                text = lines.join("\n") + "\n";
            }
            6 => {
                let mut lines: Vec<&str> = text.lines().collect();
                if lines.len() > 4 {
                    let i = 1 + rng.below(lines.len() - 3);
                    lines.swap(i, i + 1);
                }
                text = lines.join("\n") + "\n";
            }
            _ => text = destroy(rng, &text),
        }
    }
    text
}

fn destroy(rng: &mut Rng, base: &str) -> String {
    let mut bytes = base.as_bytes().to_vec();
    if bytes.is_empty() {
        return String::new();
    }
    let at = rng.below(bytes.len());
    match rng.below(5) {
        0 => {
            let n = (1 + rng.below(8)).min(bytes.len() - at);
            bytes.drain(at..at + n);
        }
        1 => {
            let frag = FRAGMENTS[rng.below(FRAGMENTS.len())];
            let tail = bytes.split_off(at);
            bytes.extend_from_slice(frag.as_bytes());
            bytes.extend_from_slice(b" ");
            bytes.extend_from_slice(&tail);
        }
        2 => {
            let n = (1 + rng.below(30)).min(bytes.len() - at);
            let chunk: Vec<u8> = bytes[at..at + n].to_vec();
            let tail = bytes.split_off(at);
            bytes.extend_from_slice(&chunk);
            bytes.extend_from_slice(&tail);
        }
        3 => bytes[at] = b" ;()[]=\"/#+-*<>0aZ_@"[rng.below(20)],
        _ => bytes.truncate(at),
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn try_program(src: &str) -> Result<(), String> {
    let cfg = Config {
        max_loop_iterations: 30,
        runtime_stack_items: 300,
        ..Config::default()
    };
    let mut env = Env::new(cfg, single("t.abl", src), Box::new(DefaultHost));
    let h = match env.pre_process("t.abl") {
        Ok(h) => h,
        Err(_) => return Ok(()),
    };
    let Ok(inst) = env.new_instance(h) else {
        return Ok(());
    };
    for _ in 0..3 {
        if env.execute(inst, &mut NullBindings).is_err() {
            break;
        }
    }
    Ok(())
}

#[test]
fn damaged_programs_never_panic() {
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(|| {
            let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
            let mut bad = Vec::new();
            let mut compiled = 0;
            let total: usize = std::env::var("ABL_FUZZ_N")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(4000);
            for n in 0..total {
                let base = BASES[n % BASES.len()];
                let src = mutate(&mut rng, base);
                let r = catch_unwind(AssertUnwindSafe(|| try_program(&src)));
                match r {
                    Ok(_) => {}
                    Err(_) => bad.push(src.clone()),
                }
                if catch_unwind(AssertUnwindSafe(|| {
                    let mut env = Env::new(
                        Config::default(),
                        single("t.abl", &src),
                        Box::new(DefaultHost),
                    );
                    env.pre_process("t.abl").is_ok()
                }))
                .unwrap_or(false)
                {
                    compiled += 1;
                }
            }
            (bad, compiled, total)
        })
        .unwrap();
    let (bad, compiled, total) = handle.join().unwrap();
    assert!(
        bad.is_empty(),
        "{} of {} damaged programs panicked, first:\n{}",
        bad.len(),
        total,
        bad.first().cloned().unwrap_or_default()
    );
    // a healthy fraction should still compile, so the fuzzer is exercising the executor too
    assert!(compiled > total / 10, "only {compiled} of {total} compiled");
}

#[test]
fn undamaged_bases_compile_and_run() {
    for (i, b) in BASES.iter().enumerate() {
        let mut env = Env::new(Config::default(), single("t.abl", b), Box::new(DefaultHost));
        let h = env
            .pre_process("t.abl")
            .unwrap_or_else(|e| panic!("base {i}: {e}"));
        let inst = env.new_instance(h).unwrap();
        for _ in 0..4 {
            env.execute(inst, &mut NullBindings)
                .unwrap_or_else(|e| panic!("base {i}: {e}"));
        }
    }
}

#[test]
fn absurd_nesting_is_an_error_not_a_crash() {
    let deep = format!(
        "module t : integer;\ncode\n return({}1{});\nendmodule.\n",
        "(".repeat(5000),
        ")".repeat(5000)
    );
    let r = std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let mut env = Env::new(
                Config::default(),
                single("t.abl", &deep),
                Box::new(DefaultHost),
            );
            env.pre_process("t.abl").map(|_| ())
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(
        r.unwrap_err().syntax_code(),
        Some(SyntaxCode::NestingTooDeep)
    );
}

fn compile_result(cfg: Config, src: &str) -> Result<(), AblError> {
    let mut env = Env::new(cfg, single("t.abl", src), Box::new(DefaultHost));
    let h = env.pre_process("t.abl")?;
    let inst = env.new_instance(h)?;
    env.execute(inst, &mut NullBindings).map(|_| ())
}

fn on_big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(f)
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn deep_statement_nesting_is_an_error() {
    let src = on_big_stack(|| {
        let n = 5000;
        let code = format!(
            "{}x = 1;{}",
            "if (true) then ".repeat(n),
            " endif;".repeat(n)
        );
        format!("module t : integer;\nvar\n integer x;\ncode\n{code}\nreturn(x);\nendmodule.\n")
    });
    let r = on_big_stack(move || compile_result(Config::default(), &src));
    assert_eq!(
        r.unwrap_err().syntax_code(),
        Some(SyntaxCode::NestingTooDeep)
    );
    // moderate nesting still compiles
    let ok = format!(
        "module t : integer;\nvar\n integer x;\ncode\n{}x = 1;{}\nreturn(x);\nendmodule.\n",
        "while (x < 1) do ".repeat(30),
        " endwhile;".repeat(30)
    );
    on_big_stack(move || compile_result(Config::default(), &ok)).unwrap();
}

#[test]
fn deep_expression_shapes_are_errors() {
    let nots = format!(
        "module t : integer;\nvar\n boolean b;\ncode\n b = {}true;\nreturn(1);\nendmodule.\n",
        "not ".repeat(100_000)
    );
    let chain = format!(
        "module t : integer;\nvar\n integer x;\ncode\n x = 0{};\nreturn(x);\nendmodule.\n",
        " + 1".repeat(100_000)
    );
    let ands = format!(
        "module t : integer;\nvar\n boolean b;\ncode\n b = true{};\nreturn(1);\nendmodule.\n",
        " and true".repeat(100_000)
    );
    for src in [nots, chain, ands] {
        let r = on_big_stack(move || compile_result(Config::default(), &src));
        assert_eq!(
            r.unwrap_err().syntax_code(),
            Some(SyntaxCode::NestingTooDeep)
        );
    }
    // a realistic long chain is fine
    let ok = format!(
        "module t : integer;\nvar\n integer x;\ncode\n x = 0{};\nreturn(x);\nendmodule.\n",
        " + 1".repeat(100)
    );
    on_big_stack(move || compile_result(Config::default(), &ok)).unwrap();
}

#[test]
fn array_dimensions_and_sizes_are_bounded() {
    let dims = |n: usize| {
        format!(
            "module t : integer;\nvar\n integer[{}] a;\ncode\nreturn(1);\nendmodule.\n",
            vec!["1"; n].join(",")
        )
    };
    assert!(compile_result(Config::default(), &dims(16)).is_ok());
    let r = on_big_stack(move || compile_result(Config::default(), &dims(5000)));
    assert_eq!(
        r.unwrap_err().syntax_code(),
        Some(SyntaxCode::NestingTooDeep)
    );

    let decl = |ty: &str, name: &str| {
        format!("module t : integer;\nvar\n {ty} {name};\ncode\nreturn(1);\nendmodule.\n")
    };
    for src in [
        decl("integer[1000000000]", "a"),
        decl("static integer[1000000000]", "a"),
        decl("eternal real[600000]", "a"),
        decl("integer[100000,100000]", "a"),
        decl("integer[2147483647]", "a"),
        decl("integer[46341,46341]", "a"),
        "module t : integer;\nconst\n N = -5;\nvar\n integer[N] a;\ncode\nreturn(1);\nendmodule.\n"
            .to_string(),
    ] {
        let e = compile_result(Config::default(), &src).unwrap_err();
        assert!(
            matches!(
                e.syntax_code(),
                Some(SyntaxCode::InvalidIndexType) | Some(SyntaxCode::TooManyLocalVariables)
            ),
            "{src}\n{e}"
        );
    }
    // exactly the per-array limit (511999 bytes: 127999 integers) is accepted, one element more is not
    compile_result(Config::default(), &decl("static integer[127999]", "a")).unwrap();
    let e = compile_result(Config::default(), &decl("static integer[128000]", "a")).unwrap_err();
    assert_eq!(e.syntax_code(), Some(SyntaxCode::InvalidIndexType));
}

#[test]
fn total_array_memory_is_capped() {
    // many eternal arrays that individually fit but together exceed the heap cap
    let mut src = String::from("module t : integer;\nvar\n");
    for i in 0..400 {
        src.push_str(&format!(" eternal integer[100000] a{i};\n"));
    }
    src.push_str("code\nreturn(1);\nendmodule.\n");
    let e = compile_result(Config::default(), &src).unwrap_err();
    assert!(e.message.contains("out of array memory"), "{e}");
    // a runtime allocation (recursion with local arrays) hits the same cap instead of exhausting memory
    let cfg = Config {
        max_heap_bytes: 100_000,
        ..Config::default()
    };
    let rec = "module t : integer;\nfunction f;\nvar\n integer[1000] big;\ncode\n f;\nendfunction;\ncode\nf;\nreturn(0);\nendmodule.\n";
    let e = on_big_stack(move || compile_result(cfg, rec)).unwrap_err();
    assert!(e.message.contains("out of array memory"), "{e}");
}
