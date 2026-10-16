//! Requested-heap probe, measured separately from walltime.

#[path = "runtime_probe/mod.rs"]
mod probe;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() -> anyhow::Result<()> {
    probe::run(true)
}
