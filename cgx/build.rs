use vergen_gix::{Emitter, GixBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Generate the git version information which will be available at compile time in env vars
    // that are used to construct the `--version` output of the binary.
    let gix = GixBuilder::default().sha(true).commit_date(true).build()?;

    Emitter::default().add_instructions(&gix)?.emit()?;

    Ok(())
}
