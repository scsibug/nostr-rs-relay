use clap::Parser;

#[derive(Parser)]
#[command(about = "A nostr relay written in Rust", author = env!("CARGO_PKG_AUTHORS"), version = env!("CARGO_PKG_VERSION"))]
pub struct CLIArgs {
    #[arg(
        short,
        long,
        help = "Use the <directory> as the location of the database",
        default_value = ".",
        required = false
    )]
    pub db: String,
}
