//! Native walltime probe using the System allocator. See benches/runtime_regression.md.

#[path = "runtime_probe/mod.rs"]
mod probe;

fn main() -> anyhow::Result<()> {
    probe::run(false)
}
