use std::collections::{HashMap, HashSet};
use std::fmt::{Error, Formatter};
use std::num::NonZeroUsize;
use std::path::PathBuf;

use clap::{App, Arg};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::export::fmt::Debug;

use crate::limits::{Category, LimitsEntry, LimitsFile, Threshold};
use crate::search_for_files::FileData;
use crate::settings::{Kind, Settings};

mod search_for_files;
mod search_in_files;
mod settings;
mod limits;
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
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
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

fn flatten_limits(
    raw_form: &HashMap<PathBuf, LimitsFile>,
) -> HashMap<LimitsEntry, u64> {
    let mut result: HashMap<LimitsEntry, u64> = HashMap::new();
    for (path, data) in raw_form {
        for (kind, entry) in data.iter() {
            match entry {
                Threshold::Number(x) => {
                    result.insert(LimitsEntry::new(Some(path), kind.clone(), Category::none()), *x);
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;

    let mut settings = config::Config::default();
    let config_file = args.config_file;

    settings.merge(config::File::from(config_file.as_path()))?;

    let mut settings_obj = settings.try_into::<Settings>()?;
    println!("{}", settings_obj.display());

    let globset = construct_types_info(&settings_obj);
    let rx = search_for_files::construct_file_searcher(&args.start_dir, globset);

    let mut log_files = Vec::with_capacity(256);
    let mut limits: HashMap<PathBuf, LimitsFile> = HashMap::new();

    for p in rx {
        match p {
            FileData::LogFile(log_file, kinds) => {
                log_files.push((log_file, kinds));
            }
            FileData::LimitsFile(path) => {
                let limit = limits::parse_limits_file(&mut settings_obj.string_arena, &path).expect("OMFG");
                limits.insert(path, limit);
            }
        }
    }

    let flat_limits = flatten_limits(&limits);

    for (path, limits_file) in &limits {
        println!("{}: {}", path.display(), limits_file.display(&settings_obj.string_arena));
    }

    let rx = search_in_files::search_files(&settings_obj, log_files, &limits);

    let mut results: HashMap<LimitsEntry, HashSet<CountsTowardsLimit>> = HashMap::new();
    for search_result_result in rx {

        let search_result = match search_result_result {
            Ok(r) => r,
            Err(log_file) => {
                eprintln!("Could not open log file '{}'", log_file.display());
                continue;
            }
        };

        let incomming_arena = search_result.string_arena;
        settings_obj.string_arena.add_all(&incomming_arena);
        for (mut limits_entry, warnings) in search_result.warnings {
            limits_entry.category.convert(&incomming_arena, &settings_obj.string_arena);
            results
                .entry(limits_entry)
                .or_insert_with(HashSet::new)
                .extend(warnings.into_iter().map(|mut w| {
                    w.category.convert(&incomming_arena, &settings_obj.string_arena);
                    w
                }));
        }
    }

    println!("{:?}", results);
    println!("{:?}", flat_limits);

    // Finally, check the results
    for (limits_entry, warnings) in results {
        let num_warnings = warnings.len() as u64;
        match flat_limits.get(&limits_entry) {
            Some(x) => {
                if num_warnings > *x {
                    eprintln!(
                        "Number of errors exceeded! (for category for {:?}/{:?}={})",
                        limits_entry.kind, limits_entry.category, *x
                    );
                } else {
                    println!(
                        "Number of warnings is under the threshold {} for: {:?}/{:?}",
                        x, limits_entry, warnings
                    );
                }
            }
            None => match flat_limits.get(&limits_entry.without_category()) {
                Some(x) => {
                    if num_warnings > *x {
                        eprintln!(
                            "Number of errors exceeded! (from blanket for {:?}={})",
                            limits_entry.kind, *x
                        );
                    } else {
                        println!(
                            "Number of warnings is under the threshold {} for: {:?}/{:?}",
                            x, limits_entry, warnings
                        );
                    }
                }
                None => {
                    let threshold = settings_obj
                        .get(&limits_entry.kind)
                        .unwrap()
                        .default
                        .unwrap_or(0);
                    if num_warnings > threshold {
                        eprintln!(
                            "Number of errors exceeded! (from default for {:?}={})",
                            limits_entry.kind, threshold
                        );
                        eprintln!("{:?}", warnings);
                    } else {
                        println!(
                            "Number of warnings is under the threshold {} for: {:?}/{:?}",
                            threshold, limits_entry, warnings
                        );
                    }
                }
            },
        }
    }
    println!("Done.");
    Ok(())
}

fn construct_types_info(settings_dict: &Settings) -> HashMap<Kind, GlobSet> {
    let mut result = HashMap::new();
    for (warning_t, warning_info) in settings_dict.iter() {
        let mut glob_builder = GlobSetBuilder::new();
        for file_glob in &warning_info.files {
            glob_builder.add(Glob::new(file_glob).expect("Bad glob pattern"));
        }
        result.insert(
            warning_t.clone(),
            glob_builder.build().expect("Could not build globset"),
        );
    }
    result
}
