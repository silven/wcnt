use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::path::PathBuf;

use clap::{App, Arg};
use crossbeam_channel::Receiver;
use env_logger;
use globset::{Glob, GlobSet, GlobSetBuilder};
use log::{debug, trace, warn};
use serde::export::fmt::Debug;
use toml;

use crate::limits::{Category, Limit, LimitsEntry, LimitsFile};
use crate::search_for_files::{FileData, LogFile};
use crate::search_in_files::LogSearchResult;
use crate::settings::{Kind, Settings};
use crate::utils::SearchableArena;
use crate::warnings::{CountsTowardsLimit, Violation};

mod limits;
mod search_for_files;
mod search_in_files;
mod settings;
mod utils;
mod warnings;

fn flatten_limits(raw_form: &HashMap<PathBuf, LimitsFile>) -> HashMap<LimitsEntry, u64> {
    let mut result: HashMap<LimitsEntry, u64> = HashMap::new();
    for (path, data) in raw_form {
        for (kind, entry) in data.iter() {
            match entry {
                Limit::Number(x) => {
                    result.insert(
                        LimitsEntry::new(Some(path), kind.clone(), Category::none()),
                        *x,
                    );
                }
                Limit::PerCategory(cats) => {
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
    verbosity: u64,
}

impl Arguments {
    fn is_verbose(&self) -> bool {
        self.verbosity > 0
    }

    fn is_very_verbose(&self) -> bool {
        self.verbosity > 1
    }
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
            Arg::with_name("verbose")
                .short("v")
                .multiple(true)
                .help("Be more verbose"),
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

    let verbosity = matches.occurrences_of("verbose");

    Ok(Arguments {
        start_dir: start_dir,
        config_file: config_file,
        verbosity: verbosity,
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
    let (log_files, limits) = collect_file_results(&mut settings.string_arena, rx)?;

    for (path, limits_file) in &limits {
        debug!("Found Limits.toml file at `{}`", path.display());
        trace!("{}", limits_file.display(&settings.string_arena));
    }

    let rx = search_in_files::search_files(&settings, &log_files, &limits);
    let results = gather_results_from_logs(&mut settings.string_arena, rx);

    // Flatten the limit entries to make it easier to match
    // Construct {limits_file}:{kind}:{category} -> u64  mapping
    let mut flat_limits = flatten_limits(&limits);
    for (kind, field) in settings.iter() {
        // Fill in the defaults from the kind settings
        let entry_for_kind_default = LimitsEntry::new(None, kind.clone(), Category::none());
        flat_limits.insert(entry_for_kind_default, field.default.unwrap_or(0));
    }

    // Finally, check the results and report any violations
    let mut violations = check_warnings_against_thresholds(&flat_limits, &results);
    violations.sort();
    if !violations.is_empty() {
        report_violations(args, &settings.string_arena, &results, &violations);
        eprintln!(
            "Found {} violations against specified limits.",
            violations.len()
        );
        std::process::exit(1);
    } else {
        Ok(())
    }
}

type LogAndLimitFiles = (Vec<LogFile>, HashMap<PathBuf, LimitsFile>);

fn collect_file_results(
    arena: &mut SearchableArena,
    rx: Receiver<FileData>,
) -> Result<LogAndLimitFiles, Box<dyn Error>> {
    let mut log_files = Vec::with_capacity(256);
    let mut limits: HashMap<PathBuf, LimitsFile> = HashMap::new();
    for file_data in rx {
        match file_data {
            FileData::LogFile(log_file) => {
                log_files.push(log_file);
            }
            FileData::LimitsFile(path) => {
                let limit = limits::parse_limits_file(arena, &path)?;
                limits.insert(path, limit);
            }
        }
    }
    Ok((log_files, limits))
}

fn report_violations(
    args: Arguments,
    arena: &SearchableArena,
    results: &HashMap<LimitsEntry, HashSet<CountsTowardsLimit>>,
    violations: &Vec<Violation>,
) {
    if args.is_verbose() {
        for v in violations {
            println!("{}", v.display(&arena));
            if args.is_very_verbose() {
                let warnings = results.get(v.entry()).expect("Got the key from here..");
                let mut warnings_vec: Vec<&CountsTowardsLimit> = Vec::with_capacity(warnings.len());
                warnings_vec.extend(warnings.iter());
                warnings_vec.sort();
                for w in &warnings_vec {
                    println!("  => {}", w.display(&arena));
                }
            }
        }
    }
}

fn gather_results_from_logs(
    arena: &mut SearchableArena,
    rx: Receiver<Result<LogSearchResult, (&LogFile, std::io::Error)>>,
) -> HashMap<LimitsEntry, HashSet<CountsTowardsLimit>> {
    let mut results: HashMap<LimitsEntry, HashSet<CountsTowardsLimit>> = HashMap::new();
    for search_result_result in rx {
        let search_result = match search_result_result {
            Ok(r) => r,
            Err((log_file, err)) => {
                warn!(
                    "Could not open log file `{}`. Reason: `{}`",
                    log_file.path().display(),
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
        limits_entry.category.remap_id(&incoming_arena, &arena);
        results
            .entry(limits_entry)
            .or_insert_with(HashSet::new)
            .extend(
                warnings
                    .into_iter()
                    .map(|w| w.remap(&incoming_arena, &arena)),
            );
    }
    results
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
                None => 0, // TODO: This means you have a warning for category you have not defined, also have no wildcard for
            },
        };

        if num_warnings > threshold {
            violations.push(Violation::new(limits_entry, threshold, num_warnings));
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

#[cfg(test)]
mod test {
    use std::num::NonZeroUsize;

    use crate::warnings::Description;

    use super::*;

    #[test]
    fn types_info_conversion_works() {
        let settings_str = r#"
        [dummy]
        regex = "^error: (?P<file>.+)"
        files = ["**/foo.txt", "**/bar.txt"]
        "#;

        let settings = toml::from_str::<Settings>(settings_str).unwrap();

        let types_info = construct_types_info(&settings).unwrap();
        let dummy_kind = Kind::new(settings.string_arena.get_id("dummy").unwrap());
        let globset = types_info.get(&dummy_kind).unwrap();
        // It should match everything that ends with [foo|bar].txt
        assert!(globset.is_match("/this/is/a/path/to/foo/foo.txt"));
        assert!(globset.is_match("/this/is/a/path/to/bar/bar.txt"));
        // It should not match anything else
        assert!(!globset.is_match("/this/is/a/path/to/foo/"));
        assert!(!globset.is_match("/etc/passwd"));
    }

    #[test]
    fn process_search_results_should_remap_the_interned_strings() {
        let mut arena_1 = SearchableArena::new();
        arena_1.insert("foo".to_owned());
        let mut arena_2 = SearchableArena::new();
        arena_2.insert("bar".to_owned());

        assert!(arena_1.get_id("bar").is_none());

        let kind = Kind::new(arena_2.insert("kind".to_owned()));
        let category = Category::new(arena_2.insert("category".to_owned()));

        let our_limit_entry = LimitsEntry::new(
            Some("/tmp/Limits.toml".as_ref()),
            kind.clone(),
            category.clone(),
        );
        let our_warning = CountsTowardsLimit::new(
            PathBuf::from("/tmp/Limits.toml"),
            Some(NonZeroUsize::new(1).unwrap()),
            Some(NonZeroUsize::new(1).unwrap()),
            kind,
            category,
            Description::none(),
        );

        let search_result = {
            let mut dict = HashMap::new();
            dict.entry(our_limit_entry)
                .or_insert_with(HashSet::new)
                .insert(our_warning);
            LogSearchResult {
                string_arena: arena_2,
                warnings: dict,
            }
        };

        let _result = process_search_results(&mut arena_1, search_result);
        assert!(arena_1.get_id("kind").is_some());
        assert!(arena_1.get_id("category").is_some());
    }
}
