use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::Formatter;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use clap::{App, Arg};
use env_logger;
use globset::{Glob, GlobSet, GlobSetBuilder};
use log::{debug, error, trace, warn};
use serde::export::fmt::Debug;
use toml;

use crate::limits::{Category, LimitsEntry, LimitsFile, Threshold};
use crate::search_for_files::FileData;
use crate::search_in_files::LogSearchResult;
use crate::settings::{Kind, Settings};
use crate::utils::SearchableArena;
use crossbeam_channel::Receiver;

mod limits;
mod search_for_files;
mod search_in_files;
mod settings;
mod utils;

#[derive(PartialEq, Eq, Hash)]
struct CountsTowardsLimit {
    culprit: PathBuf,
    line: Option<NonZeroUsize>,
    column: Option<NonZeroUsize>,
    kind: Kind,
    category: Category,
}

impl Debug for CountsTowardsLimit {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
        fn fmt_nonzero(val: Option<NonZeroUsize>) -> String {
            val.map(|x| x.to_string()).unwrap_or_else(|| "?".to_owned())
        }
        write!(
            f,
            "{}:{}:{}:[{:?}/{:?}]",
            self.culprit.display(),
            fmt_nonzero(self.line),
            fmt_nonzero(self.column),
            self.kind,
            self.category
        )
    }
}

impl CountsTowardsLimit {
    fn new(
        culprit_file: PathBuf,
        line: Option<NonZeroUsize>,
        column: Option<NonZeroUsize>,
        kind: Kind,
        category: Category,
    ) -> Self {
        CountsTowardsLimit {
            culprit: culprit_file,
            line: line,
            column: column,
            kind: kind,
            category: category,
        }
    }
}

fn flatten_limits(raw_form: &HashMap<PathBuf, LimitsFile>) -> HashMap<LimitsEntry, u64> {
    let mut result: HashMap<LimitsEntry, u64> = HashMap::new();
    for (path, data) in raw_form {
        for (kind, entry) in data.iter() {
            match entry {
                Threshold::Number(x) => {
                    result.insert(
                        LimitsEntry::new(Some(path), kind.clone(), Category::none()),
                        *x,
                    );
                }
                Threshold::PerCategory(cats) => {
                    for (cat, x) in cats {
                        result.insert(LimitsEntry::new(Some(path), kind.clone(), cat.clone()), *x);
                    }
                }
            }
        }
    }
    result
}

#[derive(Debug)]
struct Arguments {
    start_dir: PathBuf,
    config_file: PathBuf,
}

fn parse_args() -> Result<Arguments, std::io::Error> {
    let matches = App::new("wcnt - Warning Counter")
        .version(clap::crate_version!())
        .author(clap::crate_authors!())
        .about(clap::crate_description!())
        .arg(
            Arg::with_name("start_dir")
                .long("start-dir")
                .value_name("DIR")
                .help("Start search in this directory (instead of cwd)")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("config_file")
                .long("config")
                .value_name("Wcnt.toml")
                .help("Use this config file. (Instead of start-dir/Wcnt.toml)")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("v")
                .short("v")
                .multiple(true)
                .help("Sets the level of verbosity"),
        )
        .get_matches();

    let cwd = std::env::current_dir()?;
    let start_dir = matches
        .value_of_os("start_dir")
        .map(PathBuf::from)
        .unwrap_or(cwd);

    let config_file = matches
        .value_of_os("config_file")
        .map(PathBuf::from)
        .unwrap_or_else(|| start_dir.join("Wcnt.toml").to_path_buf());

    Ok(Arguments {
        start_dir: start_dir,
        config_file: config_file,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let args = parse_args()?;
    debug!("Parsed arguments `{:?}`", args);

    let mut settings: Settings = {
        let config_file = utils::read_file(args.config_file.as_path())?;
        toml::from_str(&config_file)?
    };
    debug!("Starting with these settings: {}", settings.display());

    let globset = construct_types_info(&settings)?;
    let rx = search_for_files::construct_file_searcher(&args.start_dir, globset);

    let mut log_files = Vec::with_capacity(256);
    let mut limits: HashMap<PathBuf, LimitsFile> = HashMap::new();

    for p in rx {
        match p {
            FileData::LogFile(log_file, kinds) => {
                log_files.push((log_file, kinds));
            }
            FileData::LimitsFile(path) => {
                let limit =
                    limits::parse_limits_file(&mut settings.string_arena, &path).expect("OMFG");
                limits.insert(path, limit);
            }
        }
    }

    for (path, limits_file) in &limits {
        debug!("Found Limits.toml file at `{}`", path.display());
        trace!("{}", limits_file.display(&settings.string_arena));
    }

    let rx = search_in_files::search_files(&settings, log_files, &limits);
    let results = compute_results(&mut settings.string_arena, rx);

    // Finally, check the results
    let mut flat_limits = flatten_limits(&limits);
    for (kind, field) in settings.iter() {
        let entry_for_kind_default = LimitsEntry::new(None, kind.clone(), Category::none());
        flat_limits.insert(entry_for_kind_default, field.default.unwrap_or(0));
    }

    let violations = check_warnings_against_thresholds(&flat_limits, &results);
    if !violations.is_empty() {
        Err(format!(
            "Found {} violations against specified limits.",
            violations.len()
        )
        .into())
    } else {
        println!("Done.");
        Ok(())
    }
}

fn compute_results(
    arena: &mut SearchableArena,
    rx: Receiver<Result<LogSearchResult, (PathBuf, std::io::Error)>>,
) -> HashMap<LimitsEntry, HashSet<CountsTowardsLimit>> {
    let mut results: HashMap<LimitsEntry, HashSet<CountsTowardsLimit>> = HashMap::new();
    for search_result_result in rx {
        let search_result = match search_result_result {
            Ok(r) => r,
            Err((log_file, err)) => {
                warn!(
                    "Could not open log file `{}`. Reason: `{}`",
                    log_file.display(),
                    err
                );
                // TODO: This should be a cause to abort
                continue;
            }
        };
        for (entry, warnings) in process_search_results(arena, search_result) {
            results
                .entry(entry)
                .or_insert_with(HashSet::new)
                .extend(warnings);
        }
    }
    results
}

fn process_search_results(
    arena: &mut SearchableArena,
    search_result: LogSearchResult,
) -> HashMap<LimitsEntry, HashSet<CountsTowardsLimit>> {
    let incoming_arena = search_result.string_arena;
    arena.add_all(&incoming_arena);
    let mut results = HashMap::new();

    for (mut limits_entry, warnings) in search_result.warnings {
        limits_entry.category.convert(&incoming_arena, &arena);
        results
            .entry(limits_entry)
            .or_insert_with(HashSet::new)
            .extend(warnings.into_iter().map(|mut w| {
                w.category.convert(&incoming_arena, &arena);
                w
            }));
    }
    results
}

struct Violation<'entry> {
    entry: &'entry LimitsEntry,
    threshold: u64,
    actual: u64,
}

impl<'entry> Violation<'entry> {
    fn update_threshold(mut self, threshold: u64) -> Self {
        self.threshold = threshold;
        self
    }
}

fn check_warnings_against_thresholds<'entries, 'x>(
    flat_limits: &'x HashMap<LimitsEntry, u64>,
    results: &'entries HashMap<LimitsEntry, HashSet<CountsTowardsLimit>>,
) -> Vec<Violation<'entries>> {
    let mut violations = Vec::with_capacity(flat_limits.len());
    for (limits_entry, warnings) in results {
        let num_warnings = warnings.len() as u64;
        let threshold = match flat_limits.get(&limits_entry) {
            Some(x) => *x,
            None => match flat_limits.get(&limits_entry.without_category()) {
                Some(x) => *x,
                None => {
                    error!(
                        "Could not find an entry to compare `{:?}` against",
                        limits_entry
                    );
                    0
                }
            },
        };

        if num_warnings > threshold {
            violations.push(Violation {
                entry: &limits_entry,
                threshold: threshold,
                actual: num_warnings,
            });
        }
    }
    violations
}

fn construct_types_info(
    settings_dict: &Settings,
) -> Result<HashMap<Kind, GlobSet>, Box<dyn Error>> {
    let mut result = HashMap::new();
    for (warning_t, warning_info) in settings_dict.iter() {
        let mut glob_builder = GlobSetBuilder::new();
        for file_glob in &warning_info.files {
            glob_builder.add(Glob::new(file_glob)?);
        }
        result.insert(warning_t.clone(), glob_builder.build()?);
    }
    Ok(result)
}
