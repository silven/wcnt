use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::fmt::{Error, Formatter};
use std::fs::FileType;
use std::io::Read;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use config::{ConfigError, File};
use crossbeam;
use crossbeam_channel::{bounded, Receiver};
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::{Match, Regex, RegexBuilder};
use serde::export::fmt::{Debug, Display};
use serde::{Deserialize, Deserializer, Serialize};

use crate::search_for_files::FileData;
use crate::search_for_files::FileData::Limits;
use clap::{App, Arg};

mod search_for_files;
mod search_in_files;

#[derive(Debug)]
struct MyRegex(Regex);

impl<'de> Deserialize<'de> for MyRegex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let as_str = String::deserialize(deserializer)?;
        let as_regex = RegexBuilder::new(&as_str)
            .multi_line(true)
            .build()
            .map_err(serde::de::Error::custom)?;

        for cap in as_regex.capture_names() {
            // TODO: Verify that "file" exists inside here.
        }
        Ok(MyRegex(as_regex))
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum LimitEntry {
    Number(u64),
    PerCategory(HashMap<String, u64>),
}

#[derive(Debug, Deserialize)]
struct Settings {
    regex: MyRegex,
    files: Vec<String>,
    command: Option<String>,
    default: Option<u64>,
}

#[derive(PartialEq, Eq, Hash)]
struct LimitsEntry {
    limits_file: Option<PathBuf>,
    kind: String,
    category: String,
}

impl LimitsEntry {
    fn new<T: AsRef<Path>>(limits_file: Option<T>, kind: &str, category: Option<&str>) -> Self {
        LimitsEntry {
            limits_file: limits_file.map(|x| PathBuf::from(x.as_ref())),
            kind: kind.to_owned(),
            category: category.map_or("_".to_owned(), |s| s.to_owned()),
        }
    }

    fn without_category(&self) -> Self {
        LimitsEntry {
            limits_file: self.limits_file.clone(),
            kind: self.kind.clone(),
            category: "_".to_owned(),
        }
    }
}

impl Debug for LimitsEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        match self.limits_file {
            Some(ref pb) => {
                // Silly way to take the last 3 components of the path
                let tail: PathBuf = pb
                    .components()
                    .rev()
                    .take(3)
                    .collect::<PathBuf>()
                    .components()
                    .rev()
                    .collect();
                write!(f, "..{}", tail.display());
            }
            None => {
                write!(f, "_");
            }
        };
        write!(f, ":[{}/{}]", self.kind, self.category)
    }
}

#[derive(PartialEq, Eq, Hash)]
struct CountsTowardsLimit {
    culprit: PathBuf,
    line: Option<NonZeroUsize>,
    column: Option<NonZeroUsize>,
    kind: String,
    category: String,
}

impl Debug for CountsTowardsLimit {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), Error> {
        fn fmt_nonzero(val: &Option<NonZeroUsize>) -> String {
            val.map(|x| x.to_string()).unwrap_or("?".to_owned())
        }
        write!(
            f,
            "{}:{}:{}:[{}/{}]",
            self.culprit.display(),
            fmt_nonzero(&self.line),
            fmt_nonzero(&self.column),
            self.kind,
            self.category
        )
    }
}

impl CountsTowardsLimit {
    fn new<T: AsRef<Path>>(
        culprit_file: T,
        line: Option<NonZeroUsize>,
        column: Option<NonZeroUsize>,
        kind: &str,
        category: Option<&str>,
    ) -> Self {
        CountsTowardsLimit {
            culprit: PathBuf::from(culprit_file.as_ref()),
            line: line,
            column: column,
            kind: kind.to_owned(),
            category: category.map_or("_".to_owned(), |s| s.to_owned()),
        }
    }
}

fn flatten_limits(
    raw_form: &HashMap<PathBuf, HashMap<String, LimitEntry>>,
) -> HashMap<LimitsEntry, u64> {
    let mut result: HashMap<LimitsEntry, u64> = HashMap::new();
    for (path, data) in raw_form {
        for (kind, entry) in data {
            match entry {
                LimitEntry::Number(x) => {
                    result.insert(LimitsEntry::new(Some(path), kind, None), *x);
                }
                LimitEntry::PerCategory(cats) => {
                    for (cat, x) in cats {
                        result.insert(LimitsEntry::new(Some(path), kind, Some(cat)), *x);
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

fn parse_args() -> Arguments {
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

    let cwd = std::env::current_dir()
        .expect("Could not compute current working directory!");
    let start_dir = matches
        .value_of_os("start_dir")
        .map(PathBuf::from)
        .unwrap_or(cwd);

    let config_file = matches
        .value_of_os("config_file")
        .map(PathBuf::from)
        .unwrap_or(start_dir.join("Wcnt.toml").to_path_buf());

    Arguments {
        start_dir: start_dir,
        config_file: config_file,
    }
}

fn main() {
    let args = parse_args();

    let mut settings = config::Config::default();
    let config_file = args.config_file;

    settings
        .merge(config::File::from(config_file.as_path()))
        .expect(&format!(
            "Could not read config file '{}'",
            &config_file.display()
        ));

    let settings_dict = settings
        .try_into::<HashMap<String, Settings>>()
        .expect("Could not convert the settings into a HashMap");
    println!("{:#?}", settings_dict);

    let globset = construct_types_info(&settings_dict);
    let rx = search_for_files::construct_file_searcher(&args.start_dir, globset);

    let mut log_files = Vec::with_capacity(256);
    let mut limits: HashMap<PathBuf, HashMap<String, LimitEntry>> = HashMap::new();

    for p in rx {
        match p {
            FileData::LogFile(log_file, kinds) => {
                log_files.push((log_file, kinds));
            }
            FileData::Limits(path, limit_data) => {
                limits.insert(path, limit_data);
            }
            FileData::ParseLimitsError(what, why) => {
                eprintln!("Error parsing file '{}': {}", what.display(), why);
            }
        }
    }

    let flat_limits = flatten_limits(&limits);

    let limits = Arc::new(limits);
    println!("Collected limits: {:#?}", limits);
    println!("Collected log files: {:#?}", log_files);

    let rx = search_in_files::search_files(&settings_dict, log_files, limits);

    let mut results: HashMap<LimitsEntry, HashSet<CountsTowardsLimit>> = HashMap::new();
    for (limits_entry, warning) in rx {
        results
            .entry(limits_entry)
            .or_insert_with(HashSet::new)
            .insert(warning);
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
                        "Number of errors exceeded! (for category for {}/{}={})",
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
                            "Number of errors exceeded! (from blanket for {}={})",
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
                    let threshold = settings_dict
                        .get(&limits_entry.kind)
                        .unwrap()
                        .default
                        .unwrap_or(0);
                    if num_warnings > threshold {
                        eprintln!(
                            "Number of errors exceeded! (from default for {}={})",
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
}

fn construct_types_info(settings_dict: &HashMap<String, Settings>) -> HashMap<String, GlobSet> {
    let mut result = HashMap::new();
    for (warning_t, warning_info) in settings_dict {
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
