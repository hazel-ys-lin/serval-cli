//! `serval api {list,show,find}` — inspect `.feature` specs on disk.
//!
//! - `list`: every spec under `--dir` (default `specs/`) that carries
//!   YAML frontmatter providing `api.{path,method}`. Specs without
//!   frontmatter are intentionally omitted; surfacing them is a known
//!   follow-up (see `project_open_followups.md` in this user's
//!   memory).
//! - `show <pattern>`: detail view of one spec — the api block,
//!   features, and scenarios. Pattern is a case-insensitive substring
//!   match against `api.path`, `api.method`, `api.collection`, the
//!   file path, feature names, and scenario tags. Multiple matches
//!   print the candidates and exit 3.
//! - `find <query>`: same matching as `show` but lists all
//!   API-frontmatter-having matches, like `list` with a filter.

use std::path::PathBuf;

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::cli::exit;
use crate::cli::output::OutputFormat;
use crate::error::Error;
use crate::spec::{self, SpecRecord};

#[derive(Debug, Args)]
pub struct ApiArgs {
    #[command(subcommand)]
    pub action: ApiAction,
}

#[derive(Debug, Subcommand)]
pub enum ApiAction {
    /// List `.feature` specs with API frontmatter under `--dir`.
    List(ListArgs),

    /// Show the detail of one spec resolved by a substring pattern.
    Show(ShowArgs),

    /// List specs whose frontmatter or content matches a query.
    Find(FindArgs),
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// Directory to scan for `.feature` files (recursively). Default
    /// is `specs/` in the current working directory.
    #[arg(long, default_value = "specs")]
    pub dir: PathBuf,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Substring matching the spec's `api.path`, `api.method`,
    /// `api.collection`, file path, feature name, or scenario tag.
    pub pattern: String,
    /// Directory to scan for `.feature` files (recursively). Default
    /// is `specs/` in the current working directory.
    #[arg(long, default_value = "specs")]
    pub dir: PathBuf,
}

#[derive(Debug, Args)]
pub struct FindArgs {
    /// Filter query (case-insensitive substring matched against
    /// `api.path`, `api.method`, `api.collection`, file path,
    /// feature name, and scenario tag).
    pub query: String,
    /// Directory to scan for `.feature` files (recursively). Default
    /// is `specs/` in the current working directory.
    #[arg(long, default_value = "specs")]
    pub dir: PathBuf,
}

pub fn run(args: ApiArgs, format: OutputFormat) -> i32 {
    match args.action {
        ApiAction::List(a) => run_list(a, format),
        ApiAction::Show(a) => run_show(a, format),
        ApiAction::Find(a) => run_find(a, format),
    }
}

fn map_error_to_exit(e: &Error) -> i32 {
    match e {
        Error::Spec(_) => exit::SPEC_ERROR,
        Error::System(_) | Error::Io(_) | Error::Http(_) => exit::SYSTEM_ERROR,
    }
}

fn run_list(args: ListArgs, format: OutputFormat) -> i32 {
    match spec::discover(&args.dir) {
        Ok(records) => {
            let api_specs: Vec<&SpecRecord> =
                records.iter().filter(|r| r.api().is_some()).collect();
            print_list(records.len(), &api_specs, format);
            exit::OK
        }
        Err(e) => {
            eprintln!("error: {e}");
            map_error_to_exit(&e)
        }
    }
}

fn run_find(args: FindArgs, format: OutputFormat) -> i32 {
    match spec::discover(&args.dir) {
        Ok(records) => {
            let matches: Vec<&SpecRecord> = records
                .iter()
                .filter(|r| r.api().is_some() && matches_query(r, &args.query))
                .collect();
            print_list(records.len(), &matches, format);
            exit::OK
        }
        Err(e) => {
            eprintln!("error: {e}");
            map_error_to_exit(&e)
        }
    }
}

fn run_show(args: ShowArgs, format: OutputFormat) -> i32 {
    let records = match spec::discover(&args.dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return map_error_to_exit(&e);
        }
    };

    let matches: Vec<&SpecRecord> = records
        .iter()
        .filter(|r| matches_query(r, &args.pattern))
        .collect();

    match matches.len() {
        0 => {
            eprintln!("error: no spec matches {:?}", args.pattern);
            exit::SPEC_ERROR
        }
        1 => {
            print_show(matches[0], format);
            exit::OK
        }
        n => {
            eprintln!("error: {n} specs match {:?}", args.pattern);
            eprintln!("candidates:");
            for r in matches {
                eprintln!("  {}", r.path.display());
            }
            exit::SPEC_ERROR
        }
    }
}

fn matches_query(r: &SpecRecord, query: &str) -> bool {
    let needle = query.to_lowercase();
    let path_str = r.path.to_string_lossy().to_lowercase();
    if path_str.contains(&needle) {
        return true;
    }
    if let Some(api) = r.api()
        && (api.path.to_lowercase().contains(&needle)
            || api.method.to_lowercase().contains(&needle)
            || api
                .collection
                .as_deref()
                .is_some_and(|c| c.to_lowercase().contains(&needle)))
    {
        return true;
    }
    for feature in &r.features {
        if feature.name.to_lowercase().contains(&needle) {
            return true;
        }
        for scenario in &feature.scenarios {
            for tag in &scenario.tags {
                if tag.to_lowercase().contains(&needle) {
                    return true;
                }
            }
        }
    }
    false
}

// ---------- output rendering ----------

#[derive(Serialize)]
struct ApiListing<'a> {
    source: String,
    method: &'a str,
    path: &'a str,
    collection: Option<&'a str>,
    scenario_count: usize,
    tags: Vec<&'a str>,
}

#[derive(Serialize)]
struct SpecDetail<'a> {
    source: String,
    api: Option<ApiSummary<'a>>,
    implements: Option<&'a [String]>,
    features: Vec<FeatureView<'a>>,
}

#[derive(Serialize)]
struct ApiSummary<'a> {
    path: &'a str,
    method: &'a str,
    collection: Option<&'a str>,
}

#[derive(Serialize)]
struct FeatureView<'a> {
    name: &'a str,
    scenarios: Vec<ScenarioView<'a>>,
}

#[derive(Serialize)]
struct ScenarioView<'a> {
    title: &'a str,
    tags: &'a [String],
}

fn listing_for(r: &SpecRecord) -> ApiListing<'_> {
    let api = r.api().expect("listing rows must have api frontmatter");
    ApiListing {
        source: r.path.to_string_lossy().into_owned(),
        method: &api.method,
        path: &api.path,
        collection: api.collection.as_deref(),
        scenario_count: r.scenario_count(),
        tags: r.unique_tags(),
    }
}

fn print_list(total_scanned: usize, filtered: &[&SpecRecord], format: OutputFormat) {
    match format {
        OutputFormat::Json => print_list_json(filtered),
        OutputFormat::Table => print_list_table(total_scanned, filtered),
    }
}

fn print_list_json(filtered: &[&SpecRecord]) {
    let listings: Vec<ApiListing> = filtered.iter().map(|r| listing_for(r)).collect();
    match serde_json::to_string_pretty(&listings) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("internal: serialize api list: {e}"),
    }
}

fn print_list_table(total_scanned: usize, filtered: &[&SpecRecord]) {
    if filtered.is_empty() {
        if total_scanned == 0 {
            println!("  no `.feature` files found");
        } else {
            println!(
                "  no APIs found ({total_scanned} spec file(s) scanned, none with API frontmatter)"
            );
        }
        return;
    }

    println!(
        "  {:<8}  {:<32}  {:<14}  SOURCE",
        "METHOD", "PATH", "COLLECTION"
    );
    for r in filtered {
        let api = r.api().unwrap();
        let collection = api.collection.as_deref().unwrap_or("-");
        let source = r.path.to_string_lossy();
        println!(
            "  {:<8}  {:<32}  {:<14}  {source}",
            api.method, api.path, collection
        );
    }

    println!();
    let api_count = filtered.len();
    let scanned_diff = total_scanned.saturating_sub(api_count);
    if scanned_diff > 0 {
        println!(
            "  {api_count} APIs ({total_scanned} spec file(s) scanned; {scanned_diff} \
without API frontmatter omitted)."
        );
    } else {
        println!("  {api_count} APIs in {total_scanned} spec file(s).");
    }
}

fn detail_for(r: &SpecRecord) -> SpecDetail<'_> {
    let api = r.api().map(|a| ApiSummary {
        path: &a.path,
        method: &a.method,
        collection: a.collection.as_deref(),
    });
    let implements = r.frontmatter.as_ref().and_then(|f| f.implements.as_deref());
    let features = r
        .features
        .iter()
        .map(|f| FeatureView {
            name: &f.name,
            scenarios: f
                .scenarios
                .iter()
                .map(|s| ScenarioView {
                    title: &s.title,
                    tags: &s.tags,
                })
                .collect(),
        })
        .collect();
    SpecDetail {
        source: r.path.to_string_lossy().into_owned(),
        api,
        implements,
        features,
    }
}

fn print_show(r: &SpecRecord, format: OutputFormat) {
    match format {
        OutputFormat::Json => match serde_json::to_string_pretty(&detail_for(r)) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("internal: serialize spec detail: {e}"),
        },
        OutputFormat::Table => print_show_table(r),
    }
}

fn print_show_table(r: &SpecRecord) {
    println!("  source: {}", r.path.display());
    match r.api() {
        Some(api) => {
            println!("  api:");
            println!("    path:       {}", api.path);
            println!("    method:     {}", api.method);
            if let Some(collection) = &api.collection {
                println!("    collection: {collection}");
            }
        }
        None => {
            println!("  api: (none)");
        }
    }
    if let Some(implements) = r.frontmatter.as_ref().and_then(|f| f.implements.as_deref())
        && !implements.is_empty()
    {
        println!("  implements:");
        for entry in implements {
            println!("    - {entry}");
        }
    }
    println!();
    println!("  features ({}):", r.features.len());
    for feature in &r.features {
        println!("    {}", feature.name);
        if !feature.scenarios.is_empty() {
            println!("      scenarios ({}):", feature.scenarios.len());
            for scenario in &feature.scenarios {
                let tags = if scenario.tags.is_empty() {
                    String::new()
                } else {
                    format!("[{}] ", scenario.tags.join(","))
                };
                println!("        {tags}{title}", title = scenario.title);
            }
        }
    }
}
