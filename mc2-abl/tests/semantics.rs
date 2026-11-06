//! Language semantics: each case is a complete ABL module returning an integer. The expected values were checked
//! against the original C++ interpreter built by `oracle` (`oracle-tools snippets-diff` runs every case through it;
//! set `ABL_DUMP_SNIPPETS=<dir>` to write the cases out for that tool).

mod common;

use common::*;
use mc2_abl::*;

/// A module named `t` returning an integer: `vars` go in the module `var` section, `funcs` between `var` and `code`.
fn prog(vars: &str, funcs: &str, code: &str) -> String {
    format!(
        "module t : integer;\nvar\n{vars}\n{funcs}\ncode\n{code}\nendmodule.\n",
        vars = vars,
        funcs = funcs,
        code = code
    )
}

struct Case {
    name: &'static str,
    src: String,
    expect: i32,
}

fn c(name: &'static str, vars: &str, funcs: &str, code: &str, expect: i32) -> Case {
    Case {
        name,
        src: prog(vars, funcs, code),
        expect,
    }
}

fn cases() -> Vec<Case> {
    vec![
        // ---- integer arithmetic -------------------------------------------------------------------------------
        c("int_div_trunc", "", "", "return(7 / 2);", 3),
        c("int_div_negative", "", "", "return((0 - 7) / 2);", -3),
        c("int_mod", "", "", "return(7 mod 3);", 1),
        c("int_mod_negative", "", "", "return((0 - 7) mod 3);", -1),
        c("precedence", "", "", "return(2 + 3 * 4);", 14),
        c("unary_minus_term", "", "", "return(- 2 * 3);", -6),
        c("unary_minus_then_add", "", "", "return(- 2 + 10);", 8),
        c("parens", "", "", "return((2 + 3) * 4);", 20),
        c("wraparound", "integer x;", "", "x = 2000000000; x = x + x; return(x);", -294967296),
        c("float_literal_precision", "", "", "return(16777217);", 16777216),
        c("div_by_zero_is_zero", "integer z;", "", "z = 0; return(5 / z);", 0),
        c("mod_by_zero_is_zero", "integer z;", "", "z = 0; return(5 mod z);", 0),
        // ---- real arithmetic and conversions -------------------------------------------------------------------
        c("trunc_positive", "", "", "return(trunc(7.9));", 7),
        c("trunc_negative", "", "", "return(trunc(0.0 - 7.9));", -7),
        c("round_half_up", "", "", "return(round(2.5));", 3),
        c("round_negative_half", "", "", "return(round(0.0 - 2.5));", -3),
        c("round_small", "", "", "return(round(0.4));", 0),
        c("mixed_division", "", "", "return(trunc(10 / 4.0));", 2),
        c("mixed_multiply", "", "", "return(trunc(1.5 * 4));", 6),
        c("int_assigned_to_real", "real r;", "", "r = 3; return(trunc(r * 2.5));", 7),
        c("real_literal_rounding", "real r;", "", "r = 0.1; r = r * 10.0; return(trunc(r * 1000.0));", 1000),
        c("abs_returns_real", "", "", "return(trunc(abs(0 - 3)));", 3),
        c("abs_real", "", "", "return(trunc(abs(0.0 - 2.5) * 2.0));", 5),
        c("sqrt", "", "", "return(trunc(sqrt(16)));", 4),
        c("real_div_by_zero_is_zero", "real z;", "", "z = 0.0; return(trunc(5.0 / z));", 0),
        c("real_compare_mixed", "", "", "if (3 < 3.5) then return(1); endif; return(0);", 1),
        c("real_equal_int", "", "", "if (2 == 2.0) then return(1); endif; return(0);", 1),
        // ---- booleans ------------------------------------------------------------------------------------------
        c("bool_and_or_not", "boolean a; boolean b;", "", "a = true; b = false; if ((a and not b) or b) then return(1); endif; return(0);", 1),
        c(
            "bool_no_short_circuit",
            "static integer calls;",
            "function touch : boolean;\ncode\n calls = calls + 1;\n return(false);\nendfunction;\n",
            "if (touch and touch) then return(100); endif; return(calls);",
            2,
        ),
        c("not_true", "", "", "if (not true) then return(1); endif; return(2);", 2),
        c("bool_compare", "boolean a;", "", "a = (3 > 2); if (a == true) then return(1); endif; return(0);", 1),
        // ---- chars and strings ---------------------------------------------------------------------------------
        c("char_compare", "char ch;", "", "ch = \"a\"; if (ch == \"a\") then return(1); endif; return(0);", 1),
        c("char_order", "char ch;", "", "ch = \"b\"; if (ch > \"a\") then return(1); endif; return(0);", 1),
        c("string_assign_index", "char[10] s;", "", "s = \"hello\"; if (s[1] == \"e\") then return(1); endif; return(0);", 1),
        c("string_concat", "char[20] s;", "", "s = \"ab\"; concat(s, \"cd\"); concat(s, 42); if ((s[2] == \"c\") and (s[4] == \"4\") and (s[5] == \"2\")) then return(1); endif; return(0);", 1),
        c("string_concat_real", "char[20] s;", "", "s = \"ab\"; concat(s, 2.5); if ((s[2] == \"2\") and (s[3] == \".\") and (s[4] == \"5\") and (s[5] == \"0\")) then return(1); endif; return(0);", 1),
        c("string_concat_bool", "char[20] s;", "", "s = \"ab\"; concat(s, true); if (s[2] == \"t\") then return(1); endif; return(0);", 1),
        c("string_compare_always_true", "char[6] s;", "", "s = \"hello\"; if (s == \"zzzzz\") then return(1); endif; return(0);", 1),
        // ---- arrays --------------------------------------------------------------------------------------------
        c("array_sum", "integer[5] a; integer i; integer s;", "", "for i = 0 to 4 do a[i] = i * i; endfor; s = 0; for i = 0 to 4 do s = s + a[i]; endfor; return(s);", 30),
        c("array_2d_comma", "integer[2,3] m;", "", "m[1,2] = 9; return(m[1,2]);", 9),
        c("array_2d_brackets", "integer[2,3] m;", "", "m[1][2] = 9; return(m[1,2]);", 9),
        c("array_2d_independent_rows", "integer[2,3] m;", "", "m[0,1] = 4; m[1,1] = 6; return(m[0,1] * 10 + m[1,1]);", 46),
        c("real_array", "real[3] v;", "", "v[0] = 1.5; v[1] = 2; v[2] = v[0] + v[1]; return(trunc(v[2] * 2.0));", 7),
        // ---- control flow --------------------------------------------------------------------------------------
        c("for_sum", "integer i; integer s;", "", "s = 0; for i = 1 to 10 do s = s + i; endfor; return(s);", 55),
        c("for_empty", "integer i; integer s;", "", "s = 5; for i = 3 to 1 do s = 99; endfor; return(s);", 5),
        c("for_var_after", "integer i;", "", "for i = 1 to 3 do endfor; return(i);", 3),
        c("while_loop", "integer i;", "", "i = 0; while (i < 5) do i = i + 1; endwhile; return(i);", 5),
        c("repeat_runs_once", "integer i;", "", "i = 10; repeat i = i + 1; until (true); return(i);", 11),
        c("repeat_until", "integer i;", "", "i = 0; repeat i = i + 3; until (i > 10); return(i);", 12),
        c("if_else", "integer x;", "", "x = 5; if (x > 3) then x = 1; else x = 2; endif; return(x);", 1),
        c("switch_match", "integer x;", "", "x = 2; switch x case 1: return(10); endcase; case 2: return(20); endcase; case 3: return(30); endcase; endswitch; return(0);", 20),
        c("switch_multi_label", "integer x;", "", "x = 7; switch x case 1, 7, 9: return(5); endcase; endswitch; return(0);", 5),
        c("switch_no_match", "integer x;", "", "x = 4; switch x case 1: return(10); endcase; endswitch; return(1);", 1),
        c("switch_no_fallthrough", "integer x; integer y;", "", "x = 1; y = 0; switch x case 1: y = y + 1; endcase; case 2: y = y + 10; endcase; endswitch; return(y);", 1),
        c("switch_negative_label", "integer x;", "", "x = 0 - 3; switch x case -3: return(1); endcase; endswitch; return(0);", 1),
        c("early_return_in_loop", "integer i;", "", "for i = 1 to 100 do if (i == 4) then return(i); endif; endfor; return(0);", 4),
        // ---- functions -----------------------------------------------------------------------------------------
        c(
            "recursion",
            "",
            "function fact(integer n) : integer;\ncode\n if (n <= 1) then return(1); endif;\n return(n * fact(n - 1));\nendfunction;\n",
            "return(fact(5));",
            120,
        ),
        c(
            "value_param_unchanged",
            "integer v;",
            "function bump(integer n);\ncode\n n = n + 1;\nendfunction;\n",
            "v = 5; bump(v); return(v);",
            5,
        ),
        c(
            "ref_param_changed",
            "integer v;",
            "function bump(@integer n);\ncode\n n = n + 1;\nendfunction;\n",
            "v = 5; bump(v); return(v);",
            6,
        ),
        c(
            "ref_param_array_element",
            "integer[3] a;",
            "function bump(@integer n);\ncode\n n = n + 1;\nendfunction;\n",
            "a[1] = 5; bump(a[1]); return(a[1]);",
            6,
        ),
        c(
            "ref_param_static",
            "static integer v;",
            "function bump(@integer n);\ncode\n n = n + 7;\nendfunction;\n",
            "bump(v); bump(v); return(v);",
            14,
        ),
        c(
            "real_function",
            "",
            "function half(integer x) : real;\ncode\n return(x / 2.0);\nendfunction;\n",
            "return(trunc(half(5) * 10.0));",
            25,
        ),
        c(
            "int_arg_to_real_param",
            "",
            "function twice(real x) : real;\ncode\n return(x * 2.0);\nendfunction;\n",
            "return(trunc(twice(3)));",
            6,
        ),
        c(
            "function_reads_module_var",
            "integer g;",
            "function readg : integer;\ncode\n return(g * 2);\nendfunction;\n",
            "g = 21; return(readg);",
            42,
        ),
        c(
            "function_writes_module_var",
            "integer g;",
            "function setg;\ncode\n g = 99;\nendfunction;\n",
            "setg; return(g);",
            99,
        ),
        c(
            "locals_zeroed_each_call",
            "",
            "function counter : integer;\nvar\n integer n;\ncode\n n = n + 1;\n return(n);\nendfunction;\n",
            "counter; counter; return(counter);",
            1,
        ),
        c(
            "static_in_function_persists",
            "",
            "function counter : integer;\nvar\n static integer n;\ncode\n n = n + 1;\n return(n);\nendfunction;\n",
            "counter; counter; return(counter);",
            3,
        ),
    ]
}

/// Cases that need a `const`/`type` section and therefore a hand-written module.
fn extra_cases() -> Vec<Case> {
    vec![
        Case {
            name: "enum_type_and_switch",
            src: "module t : integer;\ntype\n Color = (Red, Green, Blue);\nvar\n Color col;\ncode\n col = Green;\n switch col\n  case Red: return(10); endcase;\n  case Green: return(20); endcase;\n endswitch;\n return(0);\nendmodule.\n".into(),
            expect: 20,
        },
        Case {
            name: "enum_compare_and_values",
            src: "module t : integer;\ntype\n Color = (Red, Green, Blue);\nvar\n Color col;\ncode\n col = Blue;\n if (col == Blue) then return(1); endif;\n return(0);\nendmodule.\n".into(),
            expect: 1,
        },
        Case {
            name: "const_section",
            src: "module t : integer;\nconst\n Five = 5;\n Ten = Five;\n Neg = -Five;\n Half = 0.5;\nvar\n integer[Ten] a;\ncode\n a[Ten - 1] = Five + Neg + 4;\n return(a[4]);\nendmodule.\n".into(),
            expect: 4,
        },
        Case {
            name: "named_array_type_assign",
            src: "module t : integer;\ntype\n Pos = real[3];\nvar\n Pos a;\n Pos b;\ncode\n a[0] = 1.5; a[1] = 2.5; a[2] = 3.5;\n b = a;\n return(trunc(b[0] + b[1] + b[2]));\nendmodule.\n".into(),
            expect: 7,
        },
        Case {
            name: "array_value_param_copy_vs_ref",
            src: "module t : integer;\ntype\n IL = integer[3];\nvar\n IL a;\nfunction byval(IL x);\ncode\n x[0] = 100;\nendfunction;\nfunction byref(@IL x);\ncode\n x[1] = 200;\nendfunction;\ncode\n a[0] = 1; a[1] = 2;\n byval(a);\n byref(a);\n return(a[0] * 1000 + a[1]);\nendmodule.\n".into(),
            expect: 1200,
        },
        Case {
            name: "string_const_and_char_const",
            src: "module t : integer;\nconst\n Greeting = \"hey\";\n Letter = \"z\";\nvar\n char[8] s;\n char ch;\ncode\n s = Greeting;\n ch = Letter;\n if ((s[0] == \"h\") and (ch == \"z\")) then return(1); endif;\n return(0);\nendmodule.\n".into(),
            expect: 1,
        },
        Case {
            name: "switch_on_char_constant",
            src: "module t : integer;\nconst\n A = \"a\";\n B = \"b\";\nvar\n char ch;\ncode\n ch = \"b\";\n switch ch\n  case A: return(1); endcase;\n  case B: return(2); endcase;\n endswitch;\n return(0);\nendmodule.\n".into(),
            expect: 2,
        },
        Case {
            name: "comments_and_case_insensitivity",
            src: "MODULE T : INTEGER;\n// line comment\n/* block\n comment */ VAR\n Integer X; // trailing\nCODE\n x = 4; /* inline */ X = X + 1;\n RETURN(x);\nENDMODULE.\n".into(),
            expect: 5,
        },
        Case {
            name: "elsif_is_reserved_not_used_here_nested_if",
            src: "module t : integer;\nvar\n integer x;\ncode\n x = 3;\n if (x == 1) then\n  return(1);\n else\n  if (x == 3) then\n   return(33);\n  endif;\n endif;\n return(0);\nendmodule.\n".into(),
            expect: 33,
        },
        Case {
            name: "print_off_by_default_does_not_evaluate",
            src: "module t : integer;\nvar\n static integer calls;\nfunction f : integer;\ncode\n calls = calls + 1;\n return(7);\nendfunction;\ncode\n print(f);\n return(calls);\nendmodule.\n".into(),
            expect: 0,
        },
        Case {
            name: "print_on_directive_enables_print",
            src: "module t : integer;\nvar\n static integer calls;\nfunction f : integer;\ncode\n calls = calls + 1;\n return(7);\nendfunction;\ncode\n #print_on\n print(f);\n #print_off\n print(f);\n return(calls);\nendmodule.\n".into(),
            expect: 1,
        },
        Case {
            name: "debug_block_skipped",
            src: "module t : integer;\nvar\n integer x;\ncode\n x = 1;\n#debug_start\n x = 99;\n#debug_end\n return(x);\nendmodule.\n".into(),
            expect: 1,
        },
        Case {
            name: "debug_block_kept_with_debug_on",
            src: "module t : integer;\nvar\n integer x;\ncode\n #debug_on\n x = 1;\n#debug_start\n x = 99;\n#debug_end\n return(x);\nendmodule.\n".into(),
            expect: 99,
        },
        Case {
            name: "setflag_getflag",
            src: "module t : integer;\nvar\n integer bits;\ncode\n bits = setflag(0, 4, true);\n bits = setflag(bits, 1, true);\n bits = setflag(bits, 4, false);\n if (getflag(bits, 1) and not getflag(bits, 4)) then return(bits); endif;\n return(0 - 1);\nendmodule.\n".into(),
            expect: 1,
        },
        Case {
            name: "module_var_zeroed_each_execute_is_single_run",
            src: "module t : integer;\nvar\n integer x;\n static integer s;\ncode\n x = x + 1;\n s = s + 1;\n return(x * 10 + s);\nendmodule.\n".into(),
            expect: 11,
        },
        Case {
            name: "init_function_runs_before_code",
            src: "module t : integer;\nvar\n static integer s;\nfunction init;\ncode\n s = 40;\nendfunction;\ncode\n s = s + 2;\n return(s);\nendmodule.\n".into(),
            expect: 42,
        },
    ]
}

fn all_cases() -> Vec<Case> {
    let mut v = cases();
    v.extend(extra_cases());
    v
}

#[test]
fn semantics_table() {
    let mut failures = Vec::new();
    let dump = std::env::var_os("ABL_DUMP_SNIPPETS");
    let mut manifest = String::new();
    for case in all_cases() {
        if let Some(dir) = &dump {
            let p = std::path::Path::new(dir)
                .join("data/missions/snip")
                .join(format!("{}.abl", case.name));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, &case.src).unwrap();
            manifest.push_str(&format!("{} {}\n", case.name, case.expect));
        }
        match run_module(&case.src) {
            Ok((env, inst)) => {
                let got = env.get_integer(inst);
                if got != case.expect {
                    failures.push(format!(
                        "{}: expected {}, got {}",
                        case.name, case.expect, got
                    ));
                }
            }
            Err(e) => failures.push(format!("{}: error {}", case.name, e)),
        }
    }
    if let Some(dir) = &dump {
        std::fs::write(
            std::path::Path::new(dir).join("snippets_expected.txt"),
            manifest,
        )
        .unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn division_by_zero_can_be_made_an_error() {
    let src = prog("integer z;", "", "z = 0; return(5 / z);");
    let cfg = Config {
        division_by_zero_is_error: true,
        ..Config::default()
    };
    let mut env = Env::new(cfg, single("t.abl", &src), Box::new(DefaultHost));
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    let e = env.execute(inst, &mut NullBindings).unwrap_err();
    assert_eq!(e.runtime_code(), Some(RuntimeCode::DivisionByZero));
    assert!(e.message.contains("Division by zero"), "{}", e.message);
}
