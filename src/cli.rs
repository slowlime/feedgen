use clap::ValueHint;

use std::path::PathBuf;

#[derive(clap::Parser, Debug, Clone)]
#[command(version, about)]
pub struct Args {
    /// Path to the config file.
    ///
    /// By default, feedgen looks for a file named `feedgen.toml` in the following directories
    /// (in order):
    ///
    /// - `./` (the current directory)
    /// - `/etc`
    #[arg(
        short,
        env = "FEEDGEN_CONFIG",
        value_hint(ValueHint::FilePath)
    )]
    pub config_path: Option<PathBuf>,

    /// RSS feed server address to bind to.
    #[arg(long, env = "FEEDGEN_BIND_ADDR")]
    pub bind_addr: Option<String>,

    /// Path to the database file.
    #[arg(long, env = "FEEDGEN_DB", value_hint(ValueHint::FilePath))]
    pub db_path: Option<PathBuf>,

    /// Path to the cache directory.
    #[arg(long, env = "FEEDGEN_CACHE_DIR", value_hint(ValueHint::DirPath))]
    pub cache_dir: Option<PathBuf>,
}

impl Args {
    pub fn parse() -> Self {
        clap::Parser::parse()
    }
}
