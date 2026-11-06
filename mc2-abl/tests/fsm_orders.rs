//! Finite state machines and the order-completion protocol. The expected result lines (same format as
//! `examples/run_scripts.rs` / `oracle/oracle.cpp`) were checked against the original C++ interpreter:
//! `ABL_DUMP_EXEC=<dir> cargo test --test fsm_orders` writes the cases for `oracle-tools exec-diff`.

mod common;

use common::*;
use mc2_abl::*;

struct Case {
    name: &'static str,
    src: &'static str,
    execs: usize,
    statics: &'static [&'static str],
    /// `stmts=... ret=... state=... statics=...`
    expect: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "fsm_transition_chain",
        src: "fsm t;\nvar\n static integer count;\nstate start;\ncode\n count = count + 1;\n trans second;\nendstate;\nstate second;\ncode\n count = count + 10;\nendstate;\nendfsm.\n",
        execs: 2,
        statics: &["count"],
        expect: "stmts=3,1 ret=0 state=second statics=count:21",
    },
    Case {
        name: "fsm_transback_and_50_transition_cap",
        src: "fsm t;\nvar\n static integer n;\n static integer m;\nstate start;\ncode\n n = n + 1;\n trans other;\nendstate;\nstate other;\ncode\n m = m + 1;\n transback;\nendstate;\nendfsm.\n",
        execs: 1,
        statics: &["n", "m"],
        expect: "stmts=98 ret=0 state=other statics=n:25,m:24",
    },
    Case {
        name: "fsm_state_handles",
        src: "fsm t;\nvar\n static integer h;\n static integer cur;\nstate start;\ncode\n h = getStateHandle(\"second\");\n cur = getCurrentStateHandle;\n setState(h);\nendstate;\nstate second;\ncode\n cur = getCurrentStateHandle + 100;\nendstate;\nendfsm.\n",
        execs: 1,
        statics: &["h", "cur"],
        expect: "stmts=4 ret=0 state=second statics=h:2,cur:102",
    },
    Case {
        name: "fsm_trans_in_nested_block_keeps_running_the_block",
        src: "fsm t;\nvar\n static integer after;\n static integer later;\nstate start;\ncode\n if (true) then\n  trans second;\n  after = 1;\n endif;\n later = 1;\nendstate;\nstate second;\ncode\nendstate;\nendfsm.\n",
        execs: 1,
        statics: &["after", "later"],
        expect: "stmts=3 ret=0 state=second statics=after:1,later:0",
    },
    Case {
        name: "fsm_helper_called_after_trans_does_nothing",
        src: "fsm t;\nvar\n static integer ran;\nfunction helper;\ncode\n ran = ran + 1;\nendfunction;\nstate start;\ncode\n if (true) then\n  trans second;\n  helper;\n endif;\nendstate;\nstate second;\ncode\nendstate;\nendfsm.\n",
        execs: 1,
        statics: &["ran"],
        expect: "stmts=3 ret=0 state=second statics=ran:0",
    },
    Case {
        name: "orders_run_until_complete_then_are_skipped",
        src: "fsm t;\nvar\n static integer a;\n static integer b;\n static integer c;\norder step1 : integer;\ncode\n a = a + 1;\n return(1);\nendorder;\norder step2 : integer;\ncode\n b = b + 1;\n if (b < 3) then\n  return(0);\n endif;\n return(1);\nendorder;\nstate start;\ncode\n step1;\n step2;\n c = c + 1;\nendstate;\nendfsm.\n",
        execs: 4,
        statics: &["a", "b", "c"],
        expect: "stmts=7,5,6,3 ret=0 state=start statics=a:1,b:3,c:2",
    },
    Case {
        name: "reset_orders_reruns_completed_orders",
        src: "fsm t;\nvar\n static integer a;\n static integer c;\norder step1 : integer;\ncode\n a = a + 1;\n return(1);\nendorder;\nstate start;\ncode\n step1;\n c = c + 1;\n if (c == 2) then\n  resetOrders(0);\n endif;\nendstate;\nendfsm.\n",
        execs: 4,
        statics: &["a", "c"],
        expect: "stmts=5,4,5,3 ret=0 state=start statics=a:2,c:4",
    },
    Case {
        name: "reset_orders_scope_one_only_clears_the_calling_routine",
        src: "fsm t;\nvar\n static integer a;\n static integer b;\n static integer c;\norder one : integer;\ncode\n a = a + 1;\n return(1);\nendorder;\norder two : integer;\ncode\n b = b + 1;\n return(1);\nendorder;\nstate start;\ncode\n one;\n two;\n c = c + 1;\n if (c == 2) then\n  resetOrders(1);\n endif;\nendstate;\nendfsm.\n",
        execs: 4,
        statics: &["a", "b", "c"],
        expect: "stmts=8,5,8,4 ret=0 state=start statics=a:2,b:2,c:4",
    },
    Case {
        name: "native_orders_that_do_not_complete_end_the_state",
        src: "fsm t;\nvar\n static integer reached;\nstate start;\ncode\n corePower(true);\n reached = 1;\nendstate;\nendfsm.\n",
        execs: 1,
        statics: &["reached"],
        expect: "stmts=1 ret=0 state=start statics=reached:0",
    },
    Case {
        name: "order_called_from_plain_module_code_does_not_use_the_protocol",
        src: "module t : integer;\nvar\n static integer a;\norder step : integer;\ncode\n a = a + 1;\n return(0);\nendorder;\ncode\n step;\n step;\n return(a);\nendmodule.\n",
        execs: 1,
        statics: &["a"],
        expect: "stmts=7 ret=2 state=- statics=a:2",
    },
    Case {
        name: "init_runs_once_per_instance_not_per_execute",
        src: "module t : integer;\nvar\n static integer inits;\n static integer runs;\nfunction init;\ncode\n inits = inits + 1;\nendfunction;\ncode\n runs = runs + 1;\n return(inits * 10 + runs);\nendmodule.\n",
        execs: 3,
        statics: &["inits", "runs"],
        expect: "stmts=3,2,2 ret=13 state=- statics=inits:1,runs:3",
    },
];

fn describe(env: &mut Env, inst: InstId, stmts: &[u32], statics: &[&str]) -> String {
    let list: Vec<String> = stmts.iter().map(|s| s.to_string()).collect();
    let mut line = format!(
        "stmts={} ret={} state={}",
        list.join(","),
        env.get_integer(inst),
        env.state_name(inst).unwrap_or("-")
    );
    if !statics.is_empty() {
        let vals: Vec<String> = statics
            .iter()
            .map(|n| format!("{n}:{}", env.get_static_integer(inst, n).unwrap_or(-1)))
            .collect();
        line.push_str(&format!(" statics={}", vals.join(",")));
    }
    line
}

#[test]
fn fsm_and_order_cases() {
    let dump = std::env::var_os("ABL_DUMP_EXEC");
    let mut manifest = String::new();
    let mut failures = Vec::new();
    for c in CASES {
        if let Some(dir) = &dump {
            let p = std::path::Path::new(dir)
                .join("data/missions/exec")
                .join(format!("{}.abl", c.name));
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, c.src).unwrap();
            manifest.push_str(&format!(
                "{}\t{}#{}#{}\t{}\n",
                c.name,
                c.name,
                c.execs,
                c.statics.join(","),
                c.expect
            ));
        }
        let mut env = Env::new(
            Config::default(),
            single("t.abl", c.src),
            Box::new(DefaultHost),
        );
        let run = (|| -> Result<String, AblError> {
            let h = env.pre_process("t.abl")?;
            let inst = env.new_instance(h)?;
            let mut stmts = Vec::new();
            for _ in 0..c.execs {
                stmts.push(env.execute(inst, &mut NullBindings)?);
            }
            Ok(describe(&mut env, inst, &stmts, c.statics))
        })();
        match run {
            Ok(got) if got == c.expect => {}
            Ok(got) => failures.push(format!("{}: expected `{}` got `{}`", c.name, c.expect, got)),
            Err(e) => failures.push(format!("{}: {}", c.name, e)),
        }
    }
    if let Some(dir) = &dump {
        std::fs::write(
            std::path::Path::new(dir).join("exec_expected.txt"),
            manifest,
        )
        .unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn fsm_requires_a_start_state() {
    let mut env = Env::new(
        Config::default(),
        single("t.abl", "fsm f;\nstate other;\ncode\nendstate;\nendfsm.\n"),
        Box::new(DefaultHost),
    );
    let h = env.pre_process("t.abl").unwrap();
    let e = env.new_instance(h).unwrap_err();
    assert!(
        e.message.contains("FSM has no Start state"),
        "{}",
        e.message
    );
}

#[test]
fn transback_without_previous_state_is_a_runtime_error() {
    let mut env = Env::new(
        Config::default(),
        single(
            "t.abl",
            "fsm f;\nstate start;\ncode\n transback;\nendstate;\nendfsm.\n",
        ),
        Box::new(DefaultHost),
    );
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    let e = env.execute(inst, &mut NullBindings).unwrap_err();
    assert_eq!(e.runtime_code(), Some(RuntimeCode::NullPrevState));
}

#[test]
fn endless_state_loop_is_reported_but_not_fatal() {
    let mut env = Env::new(
        Config::default(),
        single(
            "t.abl",
            "fsm f;\nstate start;\ncode\n trans start;\nendstate;\nendfsm.\n",
        ),
        Box::new(DefaultHost),
    );
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    env.execute(inst, &mut NullBindings).unwrap();
    assert!(
        env.warnings
            .iter()
            .any(|w| w.contains("endless state loop")),
        "{:?}",
        env.warnings
    );
}

/// Bindings that count calls so the order protocol can be observed with native (game) orders.
#[derive(Default)]
struct Counting {
    power_calls: u32,
    complete_after: u32,
}

impl GameBindings for Counting {
    fn core_power(&mut self, _ctx: &mut NativeCtx<'_>, _up: bool) -> i32 {
        self.power_calls += 1;
        (self.power_calls >= self.complete_after) as i32
    }
}

#[test]
fn native_orders_follow_the_completion_protocol() {
    let src = "fsm f;\nvar\n static integer reached;\nstate start;\ncode\n corePower(true);\n reached = reached + 1;\nendstate;\nendfsm.\n";
    let mut env = Env::new(
        Config::default(),
        single("t.abl", src),
        Box::new(DefaultHost),
    );
    let h = env.pre_process("t.abl").unwrap();
    let inst = env.new_instance(h).unwrap();
    let mut b = Counting {
        power_calls: 0,
        complete_after: 3,
    };
    for _ in 0..5 {
        env.execute(inst, &mut b).unwrap();
    }
    // call 1 and 2 report "not done" and eject; call 3 completes; afterwards the order is skipped (not re-issued)
    assert_eq!(b.power_calls, 3);
    assert_eq!(env.get_static_integer(inst, "reached"), Some(3));
}
