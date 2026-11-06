//! Dev helper (rc-vqa-vertexdump): compares an instrumented C++ vertex dump with a port dump on the C++ pose
//! (`mc2_render::vertex_light_compare`): the input fields first, then the per-vertex lit colour.
//!
//! `cargo run -p mc2-render --example vertex_light_compare -- CPP_DUMP PORT_DUMP [TOLERANCE]` (tolerance in
//! channel bytes, default 1)
use mc2_render::vertex_light_compare::{parse_cpp, parse_port, report};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [cpp, port, rest @ ..] = args.as_slice() else {
        panic!("usage: vertex_light_compare CPP_DUMP PORT_DUMP [TOLERANCE]");
    };
    let tol = rest.first().map_or(1, |t| t.parse().expect("TOLERANCE"));
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    let cpp = parse_cpp(&read(cpp)).expect("C++ dump");
    let port = parse_port(&read(port)).expect("port dump");
    print!("{}", report(&cpp, &port, tol));
}
