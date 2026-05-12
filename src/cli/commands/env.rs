//! `serval env {list,show,set,remove}` — manage named environments
//! in the user's config file.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::cli::exit;
use crate::cli::output::OutputFormat;
use crate::config::{self, Config, EnvConfig};
use crate::error::Error;

#[derive(Debug, Args)]
pub struct EnvArgs {
    /// Override the config file path (defaults to
    /// `$SERVAL_CONFIG_FILE` or `~/.serval/config.toml`).
    #[arg(long, global = true)]
    pub config_file: Option<PathBuf>,

    #[command(subcommand)]
    pub action: EnvAction,
}

#[derive(Debug, Subcommand)]
pub enum EnvAction {
    /// List configured environments.
    List,

    /// Show one environment's configuration.
    Show(ShowArgs),

    /// Create or update an environment.
    Set(SetArgs),

    /// Delete an environment.
    Remove(RemoveArgs),
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct SetArgs {
    pub name: String,
    /// Base URL for this environment (e.g. `http://localhost:3000`).
    #[arg(long)]
    pub base_url: String,
    /// Mark this environment as `default_env` for `serval run`.
    #[arg(long)]
    pub make_default: bool,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    pub name: String,
}

pub fn run(args: EnvArgs, format: OutputFormat) -> i32 {
    let path = match resolve_path(args.config_file.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return map_error_to_exit(&e);
        }
    };

    match args.action {
        EnvAction::List => run_list(&path, format),
        EnvAction::Show(a) => run_show(&path, &a.name, format),
        EnvAction::Set(a) => run_set(&path, a),
        EnvAction::Remove(a) => run_remove(&path, &a.name),
    }
}

fn map_error_to_exit(e: &Error) -> i32 {
    match e {
        Error::Spec(_) => exit::SPEC_ERROR,
        Error::System(_) | Error::Io(_) | Error::Http(_) => exit::SYSTEM_ERROR,
    }
}

fn resolve_path(override_path: Option<&std::path::Path>) -> crate::error::Result<PathBuf> {
    match override_path {
        Some(p) => Ok(p.to_path_buf()),
        None => config::default_path(),
    }
}

fn run_list(path: &std::path::Path, format: OutputFormat) -> i32 {
    let cfg = match config::load(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return map_error_to_exit(&e);
        }
    };
    print_list(&cfg, format);
    exit::OK
}

fn run_show(path: &std::path::Path, name: &str, format: OutputFormat) -> i32 {
    let cfg = match config::load(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return map_error_to_exit(&e);
        }
    };
    match cfg.envs.get(name) {
        Some(env) => {
            print_show(name, env, &cfg, format);
            exit::OK
        }
        None => {
            eprintln!("error: no env named {name:?} in {}", path.display());
            exit::SPEC_ERROR
        }
    }
}

fn run_set(path: &std::path::Path, args: SetArgs) -> i32 {
    let mut cfg = match config::load(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return map_error_to_exit(&e);
        }
    };
    cfg.envs.insert(
        args.name.clone(),
        EnvConfig {
            base_url: args.base_url,
        },
    );
    if args.make_default {
        cfg.default_env = Some(args.name.clone());
    }
    if let Err(e) = config::save(path, &cfg) {
        eprintln!("error: {e}");
        return map_error_to_exit(&e);
    }
    println!("set env `{}` ({})", args.name, path.display());
    if args.make_default {
        println!("set default_env = {}", args.name);
    }
    exit::OK
}

fn run_remove(path: &std::path::Path, name: &str) -> i32 {
    let mut cfg = match config::load(path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return map_error_to_exit(&e);
        }
    };
    if cfg.envs.remove(name).is_none() {
        eprintln!("error: no env named {name:?} in {}", path.display());
        return exit::SPEC_ERROR;
    }
    if cfg.default_env.as_deref() == Some(name) {
        cfg.default_env = None;
    }
    if let Err(e) = config::save(path, &cfg) {
        eprintln!("error: {e}");
        return map_error_to_exit(&e);
    }
    println!("removed env `{name}`");
    exit::OK
}

// ---------- output rendering ----------

#[derive(Serialize)]
struct EnvListing<'a> {
    name: &'a str,
    base_url: &'a str,
    is_default: bool,
}

fn print_list(cfg: &Config, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            let view: Vec<EnvListing> = cfg
                .envs
                .iter()
                .map(|(name, env)| EnvListing {
                    name,
                    base_url: &env.base_url,
                    is_default: cfg.default_env.as_deref() == Some(name.as_str()),
                })
                .collect();
            match serde_json::to_string_pretty(&view) {
                Ok(s) => println!("{s}"),
                Err(e) => eprintln!("internal: serialize env list: {e}"),
            }
        }
        OutputFormat::Table => print_list_table(cfg),
    }
}

fn print_list_table(cfg: &Config) {
    if cfg.envs.is_empty() {
        println!("  no environments configured");
        return;
    }
    println!("  {:<16}  {:<10}  BASE URL", "NAME", "DEFAULT");
    for (name, env) in &cfg.envs {
        let mark = if cfg.default_env.as_deref() == Some(name.as_str()) {
            "yes"
        } else {
            "-"
        };
        println!("  {:<16}  {:<10}  {}", name, mark, env.base_url);
    }
}

fn print_show(name: &str, env: &EnvConfig, cfg: &Config, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            let view = EnvListing {
                name,
                base_url: &env.base_url,
                is_default: cfg.default_env.as_deref() == Some(name),
            };
            match serde_json::to_string_pretty(&view) {
                Ok(s) => println!("{s}"),
                Err(e) => eprintln!("internal: serialize env show: {e}"),
            }
        }
        OutputFormat::Table => {
            println!("  name:     {name}");
            println!("  base_url: {}", env.base_url);
            if cfg.default_env.as_deref() == Some(name) {
                println!("  default:  yes");
            }
        }
    }
}
