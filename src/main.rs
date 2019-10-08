#![deny(intra_doc_link_resolution_failure)]
//! wcnt (Warning Counter) is a small command line tool to count warnings in files, and map them
//! to declared limits. It may then return an error code if any limit is breached.
//!
//! The tool is mainly useful for code bases which did not start out with "Warnings as Errors", but
//! want to start clearing our their warnings little by little.
//!
//! Limits can be specified on a per directory tree basis with each Limits.toml file being used for
//! that subtree until a deeper, more specific Limits.toml file is encountered.
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

use clap::{App, Arg};
use crossbeam_channel::Receiver;
use env_logger;
use globset::{Glob, GlobSet, GlobSetBuilder};
use log::{debug, trace, warn};
use serde::export::fmt::Debug;
use toml;

use crate::limits::{Category, Limit, LimitsEntry, LimitsFile};
use crate::search_for_files::{FileData, LogFile};
use crate::search_in_files::LogSearchResults;
use crate::settings::{Kind, Settings};
use crate::utils::SearchableArena;
use crate::warnings::{CountsTowardsLimit, EntryCount, FinalTally};

mod limits;
mod search_for_files;
mod search_in_files;
mod settings;
pub mod utils;
mod warnings;

/// Flattens the mapping of [LimitsFile](struct.LimitsFile.html)s to a more efficient representation
/// using [Limit Entries](struct.LimitsEntry.html).
fn flatten_limits(raw_form: &HashMap<PathBuf, LimitsFile>) -> HashMap<LimitsEntry, Option<u64>> {
    let mut result: HashMap<LimitsEntry, Option<u64>> = HashMap::new();
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
/// Struct representing the command line arguments passed to program.
struct Arguments {
    start_dir: PathBuf,
    config_file: PathBuf,
    verbosity: u64,
    update_limits: bool,
}

impl Arguments {
    fn is_verbose(&self) -> bool {
        self.verbosity > 0
    }

    fn is_very_verbose(&self) -> bool {
        self.verbosity > 1
    }
}

/// Parse arguments to struct using Clap.
fn parse_args() -> Result<Arguments, std::io::Error> {
    let matches = App::new("Warning Counter (wcnt)")
        .version(clap::crate_version!())
        .author(clap::crate_authors!())
        .about(clap::crate_description!())
        .arg(
            Arg::with_name("start_dir")
                .long("start")
                .value_name("DIR")
                .help("Start search in this directory (instead of cwd)")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("config_file")
                .long("config")
                .value_name("Wcnt.toml")
                .help("Use this config file. (Instead of <start>/Wcnt.toml)")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("verbose")
                .short("v")
                .multiple(true)
                .help("Be more verbose. (Add more for more)"),
        )
        .arg(
            Arg::with_name("update_limits")
                .long("update-limits")
                .help("Update the Limit.toml files with lower values if no violations were found.")
                .takes_value(false),
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
        update_limits: matches.is_present("update_limits"),
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let args = parse_args()?;
    debug!("Parsed arguments `{:?}`", args);

    let mut settings: Settings = {
        let config_file = read_to_string(args.config_file.as_path())?;
        toml::from_str(&config_file)?
    };
    debug!("Starting with these settings: {}", settings.display());

    let globset = construct_types_info(&settings)?;
    let categorizables = settings.categorizables();
    let rx = search_for_files::construct_file_searcher(&args.start_dir, globset);
    let (log_files, limits) = collect_file_results(&mut settings.string_arena, &categorizables, rx)?;

    for (path, limits_file) in &limits {
        debug!("Found Limits.toml file at `{}`", path.display());
        trace!("{}", limits_file.display(&settings.string_arena));
    }

    let rx = search_in_files::search_files(
        &settings,
        &log_files,
        &limits
            .keys()
            .map(PathBuf::as_path)
            .collect::<HashSet<&Path>>(),
    );

    // Flatten the limit entries to make it easier to match
    // Construct {limits_file}:{kind}:{category} -> u64  mapping
    let results_tmp = gather_results_from_logs(&mut settings.string_arena, rx);
    let flat_limits = flatten_limits(&limits);
    let defaults: HashMap<&Kind, Option<u64>> =
        settings.iter().map(|(k, sf)| (k, sf.default)).collect();

    let results = remap_to_actual_limit_entries(&flat_limits, results_tmp);

    // Finally, check the results and report any violations
    let tally = check_warnings_against_thresholds(&flat_limits, &results, &defaults);
    let violations = tally.violations();
    if !violations.is_empty() {
        report_violations(
            args,
            &settings.string_arena,
            &results,
            &violations,
            &tally.non_violations(),
        );
        eprintln!(
            "Found {} violations against specified limits.",
            violations.len()
        );
        std::process::exit(1);
    } else {
        if args.update_limits {
            update_limits(&settings, &limits, &tally)?;
        }
        Ok(())
    }
}

/// Update `Limits.toml` files with new, lower limits.
fn update_limits(settings: &Settings, limits: &HashMap<PathBuf, LimitsFile>, tally: &FinalTally) -> Result<(), Box<dyn Error>>{
    let mut updated = HashSet::new();
    let mut limits_copy: HashMap<PathBuf, LimitsFile> = limits.clone();

    for entry_count in tally.non_violations() {
        let entry = entry_count.entry();
        if let Some(limits_path) = &entry.limits_file {
            let limit_file = limits_copy.get_mut(limits_path).expect("Infallible lookup");
            limit_file.update_limits(&entry_count);
            if limit_file != limits.get(limits_path).expect("Infallible lookup") {
                updated.insert(limits_path);
            }
        }
    }
    for path in updated {
        let limit_file = limits_copy.get(path).expect("Did not find copy?");
        let as_string = toml::to_string(&limit_file.as_serializable(&settings.string_arena))?;
        println!("Updating `{}`", path.display());
        std::fs::write(path, as_string)?;
    }
    Ok(())
}


/// Because the LimitEntries from the warnings use the category from the warning pass, it might
/// always map to an actual user defined warning. This pass lookup the actual warnings and ensure
/// that we have a user defined limit when doing later comparisons.
fn remap_to_actual_limit_entries(
    defined_limits: &HashMap<LimitsEntry, Option<u64>>,
    found: HashMap<LimitsEntry, HashSet<CountsTowardsLimit>>,
) -> HashMap<LimitsEntry, HashSet<CountsTowardsLimit>> {
    let mut result = HashMap::new();

    for (limit, warnings) in found {
        let key = if defined_limits.contains_key(&limit) {
            limit
        } else if defined_limits.contains_key(&limit.without_category()) {
            limit.without_category()
        }  else {
            // This is a default, we should probably handle this better
            limit
            //panic!("Did not find an appropriate entry inside {:?} for {:?}", defined_limits, limit);
        };

        result
            .entry(key)
            .or_insert_with(HashSet::new)
            .extend(warnings);
    }
    result
}

type LogAndLimitFiles = (Vec<LogFile>, HashMap<PathBuf, LimitsFile>);
/// Read from the channel producing file results and gather them up into lists.
fn collect_file_results(
    arena: &mut SearchableArena,
    categorizables: &HashSet<Kind>,
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
                let limit = limits::parse_limits_file(arena, &path, categorizables)?;
                limits.insert(path, limit);
            }
        }
    }
    Ok((log_files, limits))
}

/// Print the found [EntryCount](struct.EntryCount.html)s based on the verbosity level found in
/// [Arguments](struct.Arguments.html)
fn report_violations(
    args: Arguments,
    arena: &SearchableArena,
    results: &HashMap<LimitsEntry, HashSet<CountsTowardsLimit>>,
    violations: &[EntryCount],
    non_violations: &[EntryCount],
) {
    if args.is_very_verbose() {
        for entry in non_violations {
            println!("{}", entry.display(&arena));
        }
    }
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

/// Read from the channel producing [Log Search Result](struct.LogSearchResult.html)s and gather
/// them in sets, removing duplicates and grouping them per appropriate
/// [LimitsEntry](struct.LimitsEntry.html).
fn gather_results_from_logs(
    arena: &mut SearchableArena,
    rx: Receiver<Result<LogSearchResults, (&LogFile, std::io::Error)>>,
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

/// Process a single [Log Search Result](../search_in_files/struct.LogSearchResult.html) and gather all warnings that
/// should [count towards the limit](../warnings/struct.CountsTowardsLimit.html).
fn process_search_results(
    arena: &mut SearchableArena,
    search_result: LogSearchResults,
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

/// Check the collected [warnings](../warnings/struct.CountsTowardsLimit.html) and compare the amount of them
/// against the declared [limits](../limits/struct.LimitsEntry.html), resulting in a
/// [FinalTally](../warnings/struct.FinalTally.html).
fn check_warnings_against_thresholds<'entries, 'x>(
    flat_limits: &'x HashMap<LimitsEntry, Option<u64>>,
    results: &'entries HashMap<LimitsEntry, HashSet<CountsTowardsLimit>>,
    defaults: &HashMap<&Kind, Option<u64>>,
) -> FinalTally<'entries> {
    let mut tally = FinalTally::new(results.len());
    for (limits_entry, warnings) in results {
        let num_warnings = warnings.len() as u64;
        let threshold = match flat_limits.get(&limits_entry) {
            Some(x) => *x,
            None => defaults
                .get(&limits_entry.kind)
                .expect("We cannot have detected a warning for a kind we have no settings for.")
                .or(Some(0)),
        };
        tally.add(EntryCount::new(limits_entry, threshold, num_warnings));
    }
    tally
}

/// Gather the glob patterns from the [Settings](struct.Settings.html) and create a mapping from
/// [Kind](struct.Kind.html) to patterns. So the later search step can figure out what regexes to
/// use when searching through the file.
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
            LogSearchResults {
                string_arena: arena_2,
                warnings: dict,
            }
        };

        let _result = process_search_results(&mut arena_1, search_result);
        assert!(arena_1.get_id("kind").is_some());
        assert!(arena_1.get_id("category").is_some());
    }

    #[test]
    fn gather_results_from_logs_deduplicate_warnings_but_doesnt_remap_categories() {
        let mut main_arena = SearchableArena::new();
        let kind = Kind::new(main_arena.insert("kind".to_owned()));

        let mut first_arena = SearchableArena::new();
        let mut second_arena = SearchableArena::new();

        // First search finds both warnings
        let category_code1 = Category::new(first_arena.insert("-Wbad-code".to_owned()));
        let category_header1 = Category::new(first_arena.insert("-Wbad-interface".to_owned()));

        let desc_code1 = Description::new(first_arena.insert("Bad code".to_owned()));
        let desc_header1 = Description::new(first_arena.insert("Bad interface".to_owned()));

        // Second search finds only one
        let category_header2 = Category::new(second_arena.insert("-Wbad-interface".to_owned()));
        let desc_header2 = Description::new(second_arena.insert("Bad interface".to_owned()));


        let code_limit_entry = LimitsEntry::new(
            Some("/tmp/Limits.toml".as_ref()),
            kind.clone(),
            category_code1.clone()
        );

        let header_limit_entry1 = LimitsEntry::new(
            Some("/tmp/Limits.toml".as_ref()),
            kind.clone(),
            category_header1.clone()
        );

        let header_limit_entry2 = LimitsEntry::new(
            Some("/tmp/Limits.toml".as_ref()),
            kind.clone(),
            category_header2.clone()
        );

        // Code warning appears only in first search result
        let our_code_warning = CountsTowardsLimit::new(
            PathBuf::from("/tmp/src/code.c"),
            Some(NonZeroUsize::new(1).unwrap()),
            Some(NonZeroUsize::new(1).unwrap()),
            kind.clone(),
            category_code1.clone(),
            desc_code1.clone(),
        );

        //  Interface warning appears in both searches
        let our_header_warning1 = CountsTowardsLimit::new(
            PathBuf::from("/tmp/src/interface.h"),
            Some(NonZeroUsize::new(1).unwrap()),
            Some(NonZeroUsize::new(1).unwrap()),
            kind.clone(),
            category_header1,
            desc_header1,
        );

        let our_header_warning2 = CountsTowardsLimit::new(
            PathBuf::from("/tmp/src/interface.h"),
            Some(NonZeroUsize::new(1).unwrap()),
            Some(NonZeroUsize::new(1).unwrap()),
            kind.clone(),
            category_header2,
            desc_header2,
        );

        let search_result1 = {
            let mut dict = HashMap::new();
            dict.entry(code_limit_entry)
                .or_insert_with(HashSet::new)
                .extend(vec![our_code_warning]);
            dict.entry(header_limit_entry1)
                .or_insert_with(HashSet::new)
                .extend(vec![our_header_warning1]);
            LogSearchResults {
                string_arena: first_arena,
                warnings: dict,
            }
        };

        let search_result2 = {
            let mut dict = HashMap::new();
            dict.entry(header_limit_entry2)
                .or_insert_with(HashSet::new)
                .extend(vec![our_header_warning2]);
            LogSearchResults {
                string_arena: second_arena,
                warnings: dict,
            }
        };

        let (tx, rx) = crossbeam_channel::bounded(10);
        tx.send(Ok(search_result1)).unwrap();
        tx.send(Ok(search_result2)).unwrap();
        drop(tx);
        // Act
        let results = gather_results_from_logs(&mut main_arena, rx);

        // Assert
        let main_category_code = Category::new(main_arena.get_id("-Wbad-code").unwrap());
        let main_category_header = Category::new(main_arena.get_id("-Wbad-interface").unwrap());
        let main_desc_code = Description::new(main_arena.get_id("Bad code").unwrap());
        let main_desc_interface  = Description::new(main_arena.get_id("Bad interface").unwrap());

        let expected_code_warning = CountsTowardsLimit::new(
            PathBuf::from("/tmp/src/code.c"),
            Some(NonZeroUsize::new(1).unwrap()),
            Some(NonZeroUsize::new(1).unwrap()),
            kind.clone(),
            main_category_code.clone(),
            main_desc_code.clone(),
        );
        let expected_interface_warning = CountsTowardsLimit::new(
            PathBuf::from("/tmp/src/interface.h"),
            Some(NonZeroUsize::new(1).unwrap()),
            Some(NonZeroUsize::new(1).unwrap()),
            kind.clone(),
            main_category_header.clone(),
            main_desc_interface.clone(),
        );

        let main_code_entry = LimitsEntry::new(
            Some("/tmp/Limits.toml".as_ref()),
            kind.clone(),
            main_category_code,
        );
        let main_interface_entry = LimitsEntry::new(
            Some("/tmp/Limits.toml".as_ref()),
            kind.clone(),
            main_category_header,
        );

        // This is a problem because now we have two warning that, during lookup, will both
        // individually be compared to the user defined limit, which could be 1. Then our
        // program would do the wrong thing. The entries need to be remapped.
        let mut expected_result = HashMap::new();
        expected_result
            .entry(main_code_entry)
            .or_insert_with(HashSet::new)
            .extend(vec![expected_code_warning.clone()]);
        expected_result
            .entry(main_interface_entry)
            .or_insert_with(HashSet::new)
            .extend(vec![expected_interface_warning.clone()]);
        assert_eq!(expected_result, results);


        let defined_limit_entry = LimitsEntry::new(Some("/tmp/Limits.toml".as_ref()), kind.clone(), Category::none());
        let mut defined_limits = HashMap::new();
        defined_limits.insert(
            defined_limit_entry.clone(),
            Some(1));
        let processed_results = remap_to_actual_limit_entries(&defined_limits, results);
        assert_ne!(expected_result, processed_results);

        let mut expected_processed_results = HashMap::new();
        expected_processed_results
            .entry(defined_limit_entry)
            .or_insert_with(HashSet::new)
            .extend(vec![expected_interface_warning, expected_code_warning]);
        assert_eq!(expected_processed_results, processed_results);
    }
}
