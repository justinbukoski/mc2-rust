// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Runtime behaviour: errors, libraries, instances, host services and the game-function bridge.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use common::*;
use mc2_abl::*;

fn env_from(files: &[(&str, &str)]) -> Env {
    let mut m = MemorySource::new();
    for (p, t) in files {
        m.insert(p, *t);
    }
    Env::new(Config::default(), m, Box::new(DefaultHost))
}

fn run_err(src: &str) -> AblError {
    run_module(src)
        .err()
        .unwrap_or_else(|| panic!("expected a runtime error for:\n{src}"))
}

fn prog(vars: &str, funcs: &str, code: &str) -> String {
    format!("module t : integer;\nvar\n{vars}\n{funcs}\ncode\n{code}\nendmodule.\n")
}

#[test]
fn default_limits_match_initabl() {
    let c = Config::default();
    assert_eq!(c.max_registered_modules, 200);
    assert_eq!(c.max_static_variables, 100);
    assert_eq!(c.max_code_buffer_size, 102400);
    assert_eq!(c.max_eternal_variables, 5000);
    assert_eq!(c.max_loop_iterations, 100001);
    assert_eq!(c.runtime_stack_items, 10240);
    assert!(!c.debugger_active);
}

#[test]
fn array_index_errors_use_the_cxx_message() {
    let e = run_err(&prog(
        "integer[3] a; integer i;",
        "",
        "i = 3;\na[i] = 1;\nreturn(0);",
    ));
    assert_eq!(e.runtime_code(), Some(RuntimeCode::ValueOutOfRange));
    assert_eq!(
        e.message,
        "ABL RUNTIME ERROR t.abl [line 7] - (type 4) Value out of range\n"
    );
    let e = run_err(&prog(
        "integer[3] a; integer i;",
        "",
        "i = 0 - 1;\nreturn(a[i]);",
    ));
    assert_eq!(e.runtime_code(), Some(RuntimeCode::ValueOutOfRange));
    // second dimension is checked against its own bound
    let e = run_err(&prog("integer[2,3] a;", "", "a[1,3] = 1;\nreturn(0);"));
    assert_eq!(e.runtime_code(), Some(RuntimeCode::ValueOutOfRange));
}

#[test]
fn loop_iteration_limits() {
    let e = run_err(&prog("", "", "while (true) do\nendwhile;\nreturn(0);"));
    assert_eq!(e.runtime_code(), Some(RuntimeCode::InfiniteLoop));
    // exactly the limit is an error (the counter is compared after each pass); one fewer is fine
    let ok = prog(
        "integer i; integer n;",
        "",
        "setMaxLoops(5);\nn = 0;\nfor i = 1 to 5 do n = n + 1; endfor;\nreturn(n);",
    );
    let (env, inst) = run_module(&ok).unwrap();
    assert_eq!(env.get_integer(inst), 5);
    let e = run_err(&prog(
        "integer i; integer n;",
        "",
        "setMaxLoops(5);\nfor i = 1 to 6 do n = n + 1; endfor;\nreturn(n);",
    ));
    assert_eq!(e.runtime_code(), Some(RuntimeCode::InfiniteLoop));
    let e = run_err(&prog(
        "integer n;",
        "",
        "setMaxLoops(3);\nrepeat n = n + 1; until (n > 100);\nreturn(n);",
    ));
    assert_eq!(e.runtime_code(), Some(RuntimeCode::InfiniteLoop));
}

#[test]
fn infinite_recursion_overflows_the_stack_cleanly() {
    let src = prog(
        "",
        "function f;\ncode\n f;\nendfunction;\n",
        "f;\nreturn(0);",
    );
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run_module(&src).err().map(|e| e.runtime_code()))
        .unwrap();
    assert_eq!(
        handle.join().unwrap(),
        Some(Some(RuntimeCode::StackOverflow))
    );
}

#[test]
fn moderately_deep_recursion_works() {
    let src = prog(
        "",
        "function sum(integer n) : integer;\ncode\n if (n == 0) then return(0); endif;\n return(n + sum(n - 1));\nendfunction;\n",
        "return(sum(200));",
    );
    let handle = std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || run_module(&src).map(|(e, i)| e.get_integer(i)))
        .unwrap();
    assert_eq!(handle.join().unwrap().unwrap(), 20100);
}

#[test]
fn standard_function_errors() {
    let e = run_err(&prog(
        "real r;",
        "",
        "r = 0.0 - 4.0;\nreturn(trunc(sqrt(r)));",
    ));
    assert_eq!(e.runtime_code(), Some(RuntimeCode::InvalidFunctionArgument));
    let e = run_err(&prog("", "", "return(getFunctionHandle(\"xy\"));"));
    assert_eq!(e.runtime_code(), Some(RuntimeCode::UnimplementedFeature));
    let e = run_err(&prog("", "", "fatal(7, \"boom\");\nreturn(0);"));
    assert_eq!(e.kind, ErrorKind::Fatal);
    assert_eq!(e.message, "boom");
}

#[test]
fn assert_is_compiled_out_unless_enabled() {
    let (env, inst) = run_module(&prog("", "", "assert(false, 3, \"nope\");\nreturn(1);")).unwrap();
    assert_eq!(env.get_integer(inst), 1);
    let e = run_err(&prog(
        "",
        "",
        "#assert_on\nassert(false, 3, \"nope\");\n#assert_off\nreturn(1);",
    ));
    assert_eq!(e.message, "ABL ASSERT: [3] nope");
    let (env, inst) = run_module(&prog(
        "",
        "",
        "#assert_on\nassert(true, 3, \"fine\");\nreturn(1);",
    ))
    .unwrap();
    assert_eq!(env.get_integer(inst), 1);
}

#[test]
fn stringfuncs_off_removes_concat() {
    let (env, inst) = run_module(&prog("char[8] s;", "", "s = \"ab\";\n#stringfuncs_off\nconcat(s, \"cd\");\n#stringfuncs_on\nif (s[2] == \"c\") then return(1); endif;\nreturn(2);")).unwrap();
    assert_eq!(env.get_integer(inst), 2);
}

#[test]
fn heap_arrays_are_released_with_their_frames() {
    let src = "module t : integer;\ntype\n IL = integer[4];\nvar\n IL arr;\n integer i;\n integer s;\nfunction work(IL v) : integer;\nvar\n integer[16] tmp;\ncode\n tmp[3] = v[1];\n return(tmp[3]);\nendfunction;\ncode\n arr[1] = 2;\n for i = 1 to 50 do s = s + work(arr); endfor;\n return(s);\nendmodule.\n";
    let mut env = Env::new(
        Config::default(),
        single("t.abl", src),
        Box::new(DefaultHost),
    );
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    let before = env.live_arrays();
    env.execute(inst, &mut NullBindings).unwrap();
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(
        env.live_arrays(),
        before,
        "frames must release their array allocations"
    );
}

#[test]
fn statics_are_per_instance_and_eternals_are_shared() {
    let src = "module m : integer;\nvar\n static integer s;\n eternal integer e;\ncode\n s = s + 1;\n e = e + 10;\n return(s * 100 + e);\nendmodule.\n";
    let mut env = env_from(&[("m.abl", src)]);
    let h = env.pre_process("m.abl").unwrap();
    let a = env.new_instance(h).unwrap();
    let b = env.new_instance(h).unwrap();
    env.execute(a, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(a), 110);
    env.execute(b, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(b), 120);
    env.execute(a, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(a), 230);
    assert_eq!(env.get_static_integer(a, "s"), Some(2));
    assert_eq!(env.get_static_integer(b, "s"), Some(1));
    assert_eq!(env.eternal_integer("e"), Some(30));
    // host-side static access
    assert_eq!(env.set_static_integer(a, "s", 41), 0);
    env.execute(a, &mut NullBindings).unwrap();
    assert_eq!(env.get_static_integer(a, "s"), Some(42));
    assert_eq!(env.set_static_integer(a, "nope", 1), 1);
    assert_eq!(env.set_static_real(a, "s", 1.0), 2);
}

#[test]
fn static_arrays_can_be_loaded_by_the_host() {
    let src = "module m : integer;\nvar\n static integer[3] ints;\n static real[2] reals;\ncode\n return(ints[0] + ints[2] + trunc(reals[1]));\nendmodule.\n";
    let mut env = env_from(&[("m.abl", src)]);
    let h = env.pre_process("m.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    assert_eq!(env.set_static_integer_array(inst, "ints", &[10, 20, 30]), 0);
    assert_eq!(env.set_static_real_array(inst, "reals", &[1.5, 2.5]), 0);
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(inst), 42);
    assert_eq!(
        env.get_static_integer_array(inst, "ints", 3),
        Some(vec![10, 20, 30])
    );
    assert_eq!(
        env.get_static_real_array(inst, "reals", 2),
        Some(vec![1.5, 2.5])
    );
}

#[test]
fn module_parameters() {
    let src =
        "module m(integer a, real b) : integer;\ncode\n return(a * 10 + trunc(b));\nendmodule.\n";
    let mut env = env_from(&[("m.abl", src)]);
    let h = env.pre_process("m.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    env.execute_with(
        inst,
        &[ParamValue::Integer(4), ParamValue::Real(2.5)],
        None,
        &mut NullBindings,
    )
    .unwrap();
    assert_eq!(env.get_integer(inst), 42);
    // integer actual for a real formal is converted
    env.execute_with(
        inst,
        &[ParamValue::Integer(1), ParamValue::Integer(3)],
        None,
        &mut NullBindings,
    )
    .unwrap();
    assert_eq!(env.get_integer(inst), 13);
}

#[test]
fn execute_function_runs_init_first_and_then_only_that_function() {
    let src = "module m;\nvar\n static integer log;\nfunction init;\ncode\n log = log * 10 + 1;\nendfunction;\nfunction alarm;\ncode\n log = log * 10 + 2;\nendfunction;\ncode\n log = log * 10 + 3;\nendmodule.\n";
    let mut env = env_from(&[("m.abl", src)]);
    let h = env.pre_process("m.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    let alarm = env.find_function(inst, "Alarm", false).expect("alarm");
    env.execute_function(inst, alarm, &mut NullBindings)
        .unwrap();
    assert_eq!(env.get_static_integer(inst, "log"), Some(12));
    env.execute_function(inst, alarm, &mut NullBindings)
        .unwrap();
    assert_eq!(env.get_static_integer(inst, "log"), Some(122));
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(env.get_static_integer(inst, "log"), Some(1223));
}

const LIB: &str = "library lib;\nvar\n static integer counter;\n static integer inits;\n integer notstatic;\nfunction init;\ncode\n inits = inits + 1;\nendfunction;\nfunction bump : integer;\ncode\n counter = counter + 1;\n return(counter);\nendfunction;\nfunction badaccess : integer;\ncode\n notstatic = 1;\n return(notstatic);\nendfunction;\ncode\nendlibrary.\n";

#[test]
fn libraries_have_one_instance_with_shared_statics_and_a_lazy_init() {
    let main =
        "module m : integer;\ncode\n bump;\n bump;\n return(lib.bump * 10 + inits);\nendmodule.\n";
    let main2 = "module n : integer;\ncode\n return(bump);\nendmodule.\n";
    let mut env = env_from(&[("lib.abx", LIB), ("m.abl", main), ("n.abl", main2)]);
    let lib = env.load_library("lib.abx").unwrap().expect("loaded");
    assert!(env.is_library(lib));
    assert!(
        env.load_library("lib.abx").unwrap().is_none(),
        "a library is loaded once"
    );
    let h = env.pre_process("m.abl").unwrap();
    assert!(env.module(h).libraries_used.contains(&lib));
    let m = env.new_instance(h).unwrap();
    env.execute(m, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(m), 31); // counter 3 -> 30, init ran exactly once
    env.execute(m, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(m), 61);
    // another module shares the same library instance
    let h2 = env.pre_process("n.abl").unwrap();
    let n = env.new_instance(h2).unwrap();
    env.execute(n, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(n), 7);
}

#[test]
fn library_routines_cannot_reach_their_own_non_static_module_variables() {
    let main = "module m : integer;\ncode\n return(badaccess);\nendmodule.\n";
    let mut env = env_from(&[("lib.abx", LIB), ("m.abl", main)]);
    env.load_library("lib.abx").unwrap();
    let h = env.pre_process("m.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    let e = env.execute(inst, &mut NullBindings).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Fatal);
    assert!(e.message.contains("not reachable"), "{}", e.message);
}

#[test]
fn unknown_library_member_is_undefined() {
    let main = "module m : integer;\ncode\n return(lib.nothere);\nendmodule.\n";
    let mut env = env_from(&[("lib.abx", LIB), ("m.abl", main)]);
    env.load_library("lib.abx").unwrap();
    let e = env.pre_process("m.abl").unwrap_err();
    assert_eq!(e.syntax_code(), Some(SyntaxCode::UndefinedIdentifier));
}

#[test]
fn library_types_and_constants_are_visible_to_modules() {
    let lib = "library shapes;\nconst\n Sides = 4;\ntype\n Pos = real[3];\nfunction dummy;\ncode\nendfunction;\ncode\nendlibrary.\n";
    let main = "module m : integer;\nvar\n Pos p;\ncode\n p[2] = 2.5;\n return(Sides + trunc(p[2]));\nendmodule.\n";
    let mut env = env_from(&[("shapes.abx", lib), ("m.abl", main)]);
    env.load_library("shapes.abx").unwrap();
    let h = env.pre_process("m.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(inst), 6);
}

// ------------------------------------------------------------------------------------------------------------------
// game function bridge
// ------------------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct Game {
    global: Option<(i32, f32)>,
    shelter_range: Option<f32>,
    any: Vec<AnyValue>,
    name: String,
}

impl GameBindings for Game {
    fn get_id(&mut self, _ctx: &mut NativeCtx<'_>) -> i32 {
        77
    }
    fn get_object_position(
        &mut self,
        ctx: &mut NativeCtx<'_>,
        object_id: i32,
        coord_list: ArrayRef,
    ) {
        ctx.set_f32(coord_list, 0, object_id as f32 + 0.5);
        ctx.set_f32(coord_list, 1, 2.0);
        ctx.set_f32(coord_list, 2, 3.0);
    }
    fn get_contacts(
        &mut self,
        ctx: &mut NativeCtx<'_>,
        contact_list: ArrayRef,
        contact_criteria: i32,
        sort_criteria: i32,
    ) -> i32 {
        ctx.set_i32(contact_list, 0, 5);
        ctx.set_i32(contact_list, 1, contact_criteria * 10 + sort_criteria);
        2
    }
    fn set_global_value(&mut self, _ctx: &mut NativeCtx<'_>, variable_num: i32, value: f32) {
        self.global = Some((variable_num, value));
    }
    fn request_shelter(&mut self, _ctx: &mut NativeCtx<'_>, range: f32) -> i32 {
        self.shelter_range = Some(range);
        1
    }
    fn mc_print(&mut self, ctx: &mut NativeCtx<'_>, arg0: AnyValue) {
        if let AnyValue::CharArray(s) = arg0 {
            self.name = ctx.string(s);
        }
        self.any.push(arg0);
    }
    fn in_area(
        &mut self,
        ctx: &mut NativeCtx<'_>,
        _object_id: i32,
        area_center: ArrayRef,
        area_radius: f32,
        min_in: i32,
    ) -> bool {
        ctx.get_f32(area_center, 0) < area_radius && min_in == -1
    }
}

fn run_game(src: &str, game: &mut Game) -> (Env, InstId) {
    let mut env = Env::new(
        Config::default(),
        single("t.abl", src),
        Box::new(DefaultHost),
    );
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    env.execute(inst, game).unwrap();
    (env, inst)
}

#[test]
fn game_functions_receive_typed_arguments_and_arrays() {
    let mut g = Game::default();
    let src = prog(
        "real[3] pos; integer[4] list; integer n;",
        "",
        "getObjectPosition(7, pos);\nn = getContacts(list, 3, 4);\nif (inArea(1, pos, 100.0, -1)) then n = n + 1000; endif;\nreturn(trunc(pos[0]) * 10000 + list[1] * 10 + n + getId * 100000);",
    );
    let (env, inst) = run_game(&src, &mut g);
    // trunc(7.5)=7 -> 70000, list[1]=34 -> 340, n=2+1000, getId=77 -> 7_700_000
    assert_eq!(env.get_integer(inst), 7_700_000 + 70_000 + 340 + 1002);
}

#[test]
fn star_parameters_follow_the_cxx_callbacks() {
    let mut g = Game::default();
    let src = prog(
        "",
        "",
        "setGlobalValue(4, 1);\nreturn(requestShelter(800));",
    );
    let (_env, _inst) = run_game(&src, &mut g);
    // ABLi_popReal() on an integer actual reads the integer's bits as a float; ABLi_popIntegerReal() converts.
    assert_eq!(g.global, Some((4, f32::from_bits(1))));
    assert_eq!(g.shelter_range, Some(800.0));
    let mut g = Game::default();
    let src = prog("", "", "setGlobalValue(4, 1.5);\nreturn(0);");
    run_game(&src, &mut g);
    assert_eq!(g.global, Some((4, 1.5)));
}

#[test]
fn integer_to_real_star_parameters_are_reported_at_compile_time() {
    let src = prog("", "", "setGlobalValue(4, 1);\nreturn(0);");
    let mut env = Env::new(
        Config::default(),
        single("t.abl", &src),
        Box::new(DefaultHost),
    );
    env.pre_process("t.abl").unwrap();
    assert!(
        env.warnings.iter().any(|w| w.contains("setglobalvalue")),
        "{:?}",
        env.warnings
    );
}

#[test]
fn anything_parameters_carry_their_type() {
    let mut g = Game::default();
    let src = prog("char[8] s;", "", "s = \"hey\";\nmcPrint(s);\nmcPrint(5);\nmcPrint(2.5);\nmcPrint(true);\nmcPrint(\"x\");\nreturn(0);");
    run_game(&src, &mut g);
    assert_eq!(g.name, "hey");
    assert_eq!(
        &g.any[1..],
        &[
            AnyValue::Int(5),
            AnyValue::Real(2.5),
            AnyValue::Bool(true),
            AnyValue::Char(b'x')
        ]
    );
}

#[test]
fn game_function_signatures_are_type_checked() {
    let e = compile_err(&prog("", "", "return(getContacts(1, 2, 3));"));
    assert_eq!(e, SyntaxCode::IncompatibleTypes);
    let e = compile_err(&prog("", "", "return(getId(1));"));
    assert_eq!(e, SyntaxCode::WrongNumberOfParams);
    let e = compile_err(&prog("real r;", "", "r = 1.0;\nreturn(selectObject(r));"));
    assert_eq!(e, SyntaxCode::IncompatibleTypes);
}

fn compile_err(src: &str) -> SyntaxCode {
    let mut env = Env::new(
        Config::default(),
        single("t.abl", src),
        Box::new(DefaultHost),
    );
    env.pre_process("t.abl").unwrap_err().syntax_code().unwrap()
}

#[test]
fn registry_has_every_game_function() {
    assert_eq!(NATIVES.len(), 230);
    let names: std::collections::HashSet<_> = NATIVES.iter().map(|n| n.name).collect();
    assert_eq!(names.len(), NATIVES.len(), "script names are unique");
    for expect in [
        "getid",
        "ordermoveto",
        "newmoveto",
        "coremoveto",
        "coreeject",
        "freezegui",
        "getmapinfo",
        "settimer",
    ] {
        assert!(names.contains(expect), "{expect}");
    }
    let order_count = NATIVES.iter().filter(|n| n.is_order).count();
    assert_eq!(order_count, 15);
}

#[test]
fn host_closures_can_be_registered_as_game_functions() {
    let calls = Rc::new(RefCell::new(Vec::new()));
    let seen = calls.clone();
    let src = prog(
        "integer[3] a;",
        "",
        "a[0] = 4; a[1] = 5;\nreturn(hostSum(a, 2, 1.5) + 1);",
    );
    let mut env = Env::new(
        Config::default(),
        single("t.abl", &src),
        Box::new(DefaultHost),
    );
    env.add_function("hostSum", false, "Iir", "i", move |ctx, args| {
        let arr = args[0].array();
        let n = args[1].int() as usize;
        let extra = args[2].real();
        seen.borrow_mut().push(n);
        let mut sum = 0;
        for i in 0..n {
            sum += ctx.get_i32(arr, i);
        }
        NativeRet::Int(sum + extra as i32)
    });
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(inst), 11);
    assert_eq!(*calls.borrow(), vec![2]);
}

#[test]
fn skipped_game_orders_still_evaluate_their_arguments() {
    // A completed native order is not re-issued, but its arguments are still evaluated.
    let src = "fsm f;\nvar\n static integer evals;\nfunction touch : boolean;\ncode\n evals = evals + 1;\n return(true);\nendfunction;\nstate start;\ncode\n corePower(touch);\nendstate;\nendfsm.\n";
    #[derive(Default)]
    struct B {
        calls: u32,
    }
    impl GameBindings for B {
        fn core_power(&mut self, _ctx: &mut NativeCtx<'_>, _up: bool) -> i32 {
            self.calls += 1;
            1
        }
    }
    let mut env = env_from(&[("t.abl", src)]);
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    let mut b = B::default();
    for _ in 0..3 {
        env.execute(inst, &mut b).unwrap();
    }
    assert_eq!(b.calls, 1);
    assert_eq!(env.get_static_integer(inst, "evals"), Some(3));
}

// ------------------------------------------------------------------------------------------------------------------
// host services
// ------------------------------------------------------------------------------------------------------------------

#[derive(Default)]
struct RecordingHost {
    log: Rc<RefCell<Vec<String>>>,
    next_random: i32,
}

impl AblHost for RecordingHost {
    fn random(&mut self, range: i32) -> i32 {
        self.log.borrow_mut().push(format!("random({range})"));
        self.next_random
    }
    fn seed_random(&mut self, seed: u32) {
        self.log.borrow_mut().push(format!("seed({seed})"));
    }
    fn debug_print(&mut self, msg: &str) {
        self.log.borrow_mut().push(format!("print:{msg}"));
    }
    fn user_file_open(&mut self, handle: usize, name: &str) -> bool {
        self.log.borrow_mut().push(format!("open {handle} {name}"));
        true
    }
    fn user_file_write(&mut self, handle: usize, line: &str) {
        self.log.borrow_mut().push(format!("write {handle} {line}"));
    }
    fn user_file_close(&mut self, handle: usize) {
        self.log.borrow_mut().push(format!("close {handle}"));
    }
}

#[test]
fn random_print_and_user_files_go_through_the_host() {
    let log = Rc::new(RefCell::new(Vec::new()));
    let host = RecordingHost {
        log: log.clone(),
        next_random: 3,
    };
    let src = prog(
        "integer f; char[8] s;",
        "",
        "seedRandom(9);\n#print_on\nprint(\"hello\");\nprint(12);\nprint(1.5);\n#print_off\nf = fileOpen(\"out.log\");\nfileWrite(f, \"line\");\nfileClose(f);\nreturn(random(10) + f);",
    );
    let mut env = Env::new(Config::default(), single("t.abl", &src), Box::new(host));
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(inst), 3);
    assert_eq!(
        *log.borrow(),
        vec![
            "seed(9)",
            "print:hello",
            "print:12",
            "print:1.5000",
            "open 0 out.log",
            "write 0 line",
            "close 0",
            "random(10)"
        ]
    );
}

#[test]
fn seeded_host_is_deterministic() {
    let mut a = SeededHost::seeded(7);
    let mut b = SeededHost::seeded(7);
    let xs: Vec<i32> = (0..20).map(|_| a.random(100)).collect();
    let ys: Vec<i32> = (0..20).map(|_| b.random(100)).collect();
    assert_eq!(xs, ys);
    assert!(xs.iter().all(|&x| (0..100).contains(&x)));
    assert!(xs.iter().any(|&x| x != xs[0]));
}

#[test]
fn state_can_be_saved_and_restored() {
    let fsm = "fsm f;\nvar\n static integer n;\n static integer[3] arr;\n eternal integer e;\n eternal integer[2] ea;\nstate start;\ncode\n n = n + 1;\n arr[1] = arr[1] + 5;\n e = e + 100;\n ea[0] = n;\n trans next;\nendstate;\nstate next;\ncode\n n = n + 10;\nendstate;\nendfsm.\n";
    let build = || {
        let mut env = env_from(&[("f.abl", fsm)]);
        let h = env.pre_process("f.abl").unwrap();
        let inst = env.new_instance(h).unwrap();
        (env, inst)
    };
    let (mut env, inst) = build();
    env.execute(inst, &mut NullBindings).unwrap();
    let saved = env.save_state();
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(env.get_static_integer(inst, "n"), Some(21));
    assert_eq!(env.state_name(inst), Some("next"));

    // A freshly built environment restores to the saved point and continues identically.
    let (mut env2, inst2) = build();
    env2.load_state(&saved).unwrap();
    assert_eq!(env2.get_static_integer(inst2, "n"), Some(11));
    assert_eq!(
        env2.get_static_integer_array(inst2, "arr", 3),
        Some(vec![0, 5, 0])
    );
    assert_eq!(env2.eternal_integer("e"), Some(100));
    assert_eq!(env2.state_name(inst2), Some("next"));
    assert_eq!(env2.prev_state_name(inst2), Some("start"));
    env2.execute(inst2, &mut NullBindings).unwrap();
    assert_eq!(env2.get_static_integer(inst2, "n"), Some(21));

    // Mismatched environments are rejected.
    let mut other = env_from(&[("g.abl", "module g;\ncode\nendmodule.\n")]);
    other.pre_process("g.abl").unwrap();
    assert!(other.load_state(&saved).is_err());
    assert!(other.load_state(b"junk").is_err());
}

#[test]
fn init_with_declared_parameters_gets_zeroed_arguments() {
    let src = "module m : integer;\nvar\n static integer seen;\nfunction init(integer x, real y);\ncode\n seen = x + 5 + trunc(y);\nendfunction;\ncode\n return(seen);\nendmodule.\n";
    let mut env = env_from(&[("m.abl", src)]);
    let h = env.pre_process("m.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(env.get_integer(inst), 5);
}

#[test]
fn saved_state_keeps_instance_names_and_forgets_completed_orders() {
    // ablenv.cpp: ABLModule::write stores the name, FSM states, init flag and statics; ABLModule::read allocates the
    // order-call flags zeroed, so completed orders are issued again after a load.
    let src = "fsm f;\nvar\n static integer runs;\norder once : integer;\ncode\n runs = runs + 1;\n return(1);\nendorder;\nstate start;\ncode\n once;\nendstate;\nendfsm.\n";
    let build = || {
        let mut env = env_from(&[("f.abl", src)]);
        let h = env.pre_process("f.abl").unwrap();
        let inst = env.new_instance(h).unwrap();
        (env, inst)
    };
    let (mut env, inst) = build();
    env.set_name(inst, "Brain");
    env.execute(inst, &mut NullBindings).unwrap();
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(
        env.get_static_integer(inst, "runs"),
        Some(1),
        "completed order is skipped"
    );
    let saved = env.save_state();

    let (mut env2, inst2) = build();
    assert_eq!(env2.name(inst2), "");
    env2.load_state(&saved).unwrap();
    assert_eq!(env2.name(inst2), "Brain");
    assert_eq!(env2.get_static_integer(inst2, "runs"), Some(1));
    env2.execute(inst2, &mut NullBindings).unwrap();
    assert_eq!(
        env2.get_static_integer(inst2, "runs"),
        Some(2),
        "order flags are not restored"
    );

    // loading into the same environment clears the flags too
    env.load_state(&saved).unwrap();
    env.execute(inst, &mut NullBindings).unwrap();
    assert_eq!(env.get_static_integer(inst, "runs"), Some(2));
}

#[derive(Default)]
struct Compat {
    infantry: Option<(i32, i32, f32, i32)>,
    sorted: Option<(f32, i32, i32)>,
    salvage: Option<(i32, i32)>,
}

impl GameBindings for Compat {
    // execCreateInfantry pops (team, commander, real[] pos, building) although it was registered as "Ri".
    fn create_infantry(
        &mut self,
        ctx: &mut NativeCtx<'_>,
        team_id: i32,
        commander_id: i32,
        world_pos: ArrayRef,
        building_part_id: i32,
    ) -> i32 {
        self.infantry = Some((
            team_id,
            commander_id,
            ctx.get_f32(world_pos, 1),
            building_part_id,
        ));
        0
    }
    // execSortWeapons pops (int[] list, size, sortType) and pushes nothing.
    fn sort_weapons(
        &mut self,
        ctx: &mut NativeCtx<'_>,
        weapon_list: ArrayRef,
        list_size: i32,
        sort_type: i32,
    ) {
        self.sorted = Some((ctx.get_i32(weapon_list, 0) as f32, list_size, sort_type));
    }
    // execSetSalvage only consumes two of its three registered arguments.
    fn set_salvage(&mut self, _ctx: &mut NativeCtx<'_>, object_id: i32, component: i32) -> bool {
        self.salvage = Some((object_id, component));
        true
    }
}

#[test]
fn callbacks_that_disagree_with_their_registration_follow_the_callback() {
    let src = prog(
        "real[3] pos; integer[4] list; integer n;",
        "",
        "pos[1] = 2.5; list[0] = 9;\nn = createInfantry(1, 2, pos, 3);\nsortWeapons(list, 4, 5);\nif (setSalvage(7, 8, 99)) then n = 1; endif;\nreturn(n);",
    );
    let mut g = Compat::default();
    let mut env = Env::new(
        Config::default(),
        single("t.abl", &src),
        Box::new(DefaultHost),
    );
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    env.execute(inst, &mut g).unwrap();
    assert_eq!(g.infantry, Some((1, 2, 2.5, 3)));
    assert_eq!(g.sorted, Some((9.0, 4, 5)));
    // the third registered argument of setSalvage is evaluated (and type checked) but not passed on
    assert_eq!(g.salvage, Some((7, 8)));
    assert_eq!(env.get_integer(inst), 1);
    // the old two-argument sortWeapons registration no longer type-checks
    let bad = prog("integer[4] list;", "", "sortWeapons(list, 4);\nreturn(0);");
    assert_eq!(compile_err(&bad), SyntaxCode::MissingComma);
}
