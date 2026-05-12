//! `serval config {path,show}` — quick inspection of the user's
//! config file. `path` prints the resolved file path (useful in
//! shells: `vim "$(serval config path)"`); `show` prints the loaded
//! `Config` so users can see what `--env` lookups will resolve.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::cli::exit;
use crate::cli::output::OutputFormat;
use crate::config;
use crate::error::Error;

#[derive(Debug, Args)]
pub struct ConfigArgs {
    /// Override the config file path (defaults to
    /// `$SERVAL_CONFIG_FILE` or `~/.serval/config.toml`).
    #[arg(long, global = true)]
    pub config_file: Option<PathBuf>,

    #[command(subcommand)]
    pub action: ConfigAction,
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the resolved config file path.
    Path,
    /// Print the loaded config contents.
    Show,
}

pub fn run(args: ConfigArgs, format: OutputFormat) -> i32 {
    let path = match args.config_file {
        Some(p) => p,
        None => match config::default_path() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: {e}");
                return map_error_to_exit(&e);
            }
        },
    };

    match args.action {
        ConfigAction::Path => {
            println!("{}", path.display());
            exit::OK
        }
        ConfigAction::Show => match config::load(&path) {
            Ok(cfg) => {
                match format {
                    OutputFormat::Json => match serde_json::to_string_pretty(&cfg) {
                        Ok(s) => println!("{s}"),
                        Err(e) => eprintln!("internal: serialize config: {e}"),
                    },
                    OutputFormat::Table => {
                        println!("  path: {}", path.display());
                        match toml::to_string_pretty(&cfg) {
                            Ok(s) => {
                                println!();
                                println!("{s}");
                            }
                            Err(e) => eprintln!("internal: serialize TOML: {e}"),
                        }
                    }
                }
                exit::OK
            }
            Err(e) => {
                eprintln!("error: {e}");
                map_error_to_exit(&e)
            }
        },
    }
}

fn map_error_to_exit(e: &Error) -> i32 {
    match e {
        Error::Spec(_) => exit::SPEC_ERROR,
        Error::System(_) | Error::Io(_) | Error::Http(_) => exit::SYSTEM_ERROR,
    }
}
