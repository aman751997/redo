use anyhow::Result;

fn main() -> Result<()> {
    redo::panic_hook::install();
    println!("redo v{}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
