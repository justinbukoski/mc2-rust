//! Shared GPU-test gate: a missing adapter fails the test unless `MC2_SKIP_GPU_TESTS=1`.

/// Prints past the test harness's output capture so the gate log shows the GPU evidence.
#[allow(unused_macros)]
macro_rules! gate_println {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;

/// The opt-out decision for a `MC2_SKIP_GPU_TESTS` value: only exactly `1` skips.
pub fn skip_decision(var: Option<&std::ffi::OsStr>) -> bool {
    var.is_some_and(|v| v == "1")
}

/// True only when the environment explicitly opts out of GPU tests.
pub fn skip_requested() -> bool {
    skip_decision(std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref())
}

/// A headless device, or `None` after printing one SKIPPED line when `MC2_SKIP_GPU_TESTS=1` and
/// the device cannot be created. Any failure without that opt-out panics.
pub fn headless_or_skip(test: &str, cfg: &RenderConfig) -> Option<Gpu> {
    match Gpu::headless(cfg) {
        Ok(g) => {
            gate_println!(
                "GPU {test}: adapter {} ({}), {} samples",
                g.adapter_name,
                g.backend,
                g.samples
            );
            Some(g)
        }
        Err(e) if skip_requested() && e.to_string().contains("adapter") => {
            gate_println!("SKIPPED {test}: MC2_SKIP_GPU_TESTS=1 and no usable GPU ({e})");
            None
        }
        Err(e) => panic!("{test}: no usable GPU adapter ({e}); set MC2_SKIP_GPU_TESTS=1 to skip"),
    }
}

/// Shared test helpers: link helpers for cross-device fallback.
pub mod link_helper;
