//! Module responsible for structures and functions related to the Wcnt.toml settings file.
//! This settings file declares all [Kind](struct.Kind.html)s of warnings we are interested in.
//! For every Kind, we need a regular expression matching the warning, and a list of glob patterns
//! to know which log files we should search through.
use std::borrow::Cow;
use std::collections::{HashSet, HashMap};
use std::fmt::Display;

use id_arena::Id;
use linked_hash_map::LinkedHashMap;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Deserializer};

use crate::utils;
use crate::utils::SearchableArena;

#[derive(Debug, PartialEq, Eq, Ord, PartialOrd, Hash, Clone)]
/// A Kind is a kind of warnings, all matchable with the same regular expression.
pub(crate) struct Kind(Id<String>);

impl Kind {
    pub fn new(id: Id<String>) -> Self {
        Kind(id)
    }

    pub fn to_str<'arena>(&self, arena: &'arena SearchableArena) -> &'arena str {
        arena.lookup(self.0).unwrap()
    }
}

#[derive(Debug)]
/// Structure representing the Wcnt.toml file. To save space, we use a
/// [string arena](../utils/struct.SearchableArena.html) to store IDs instead of strings whenever
/// possible.
pub(crate) struct Settings {
    pub(crate) string_arena: SearchableArena,
    inner: LinkedHashMap<Kind, SettingsField>,
    kinds_to_ignore: HashSet<Kind>,
}

pub(crate) struct RelevantRegexes {
    inner: HashMap<Kind, Regex>,
}

impl RelevantRegexes {
    pub(crate) fn get(&self, kind: &Kind) -> Option<&Regex> {
        self.inner.get(kind)
    }
}

impl Settings {
    pub fn iter(&self) -> impl Iterator<Item = (&Kind, &SettingsField)> {
        self.inner.iter()
    }

    pub(crate) fn kinds_and_regex(&self) -> RelevantRegexes {
        RelevantRegexes {
            inner: self.kinds().map(|k|
                (k.clone(), self.inner.get(k).unwrap().regex.clone())
            ).collect(),
        }
    }

    pub(crate) fn categorizables(&self) -> HashSet<Kind> {
        let mut result = HashSet::new();
        for (kind, sf) in self.iter() {
            if sf.categorizable {
                result.insert(kind.clone());
            }
        }
        result
    }

    pub fn kinds<'me>(&'me self) -> impl Iterator<Item=&'me Kind> + 'me {
        self.inner.keys().filter(move |k| !self.should_skip_kind(k))
    }

    pub fn configure_kinds_to_run(&mut self, kinds_to_ignore: &Option<Vec<String>>) {
        if let Some(only_these) = kinds_to_ignore {
            let as_kinds: HashSet<Kind> = only_these.iter().map(|k| self.string_arena.get_id(k)).flatten().map(Kind::new).collect();
            let tmp = self.kinds().filter(|k| !as_kinds.contains(k)).cloned();
            self.kinds_to_ignore = tmp.collect();
        }
    }

    fn should_skip_kind(&self, kind: &Kind) -> bool {
        self.kinds_to_ignore.contains(kind)
    }

    pub fn display<'me>(&'me self) -> impl Display + 'me {
        utils::fmt_helper(move |f| {
            writeln!(f, "Settings {{")?;
            for (kind, field) in &self.inner {
                writeln!(f, "[{}]", kind.to_str(&self.string_arena))?;
                writeln!(f, "regex = {:?}", field.regex)?;
                writeln!(f, "files = [{}]", field.files.join(", "))?;
            }
            write!(f, "}}")
        })
    }
}

#[derive(Debug)]
/// Represents the settings required to find files, and search in those files for warnings, related
/// to a specific [Kind](struct.Kind.html)
pub(crate) struct SettingsField {
    pub(crate) regex: Regex,
    pub(crate) files: Vec<String>,
    categorizable: bool,
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = <LinkedHashMap<String, SettingsField>>::deserialize(deserializer)?;
        let mut result = LinkedHashMap::new();
        let mut string_arena = SearchableArena::new();
        for (key, val) in raw.into_iter() {
            let captures: HashSet<&str> = val.regex.capture_names().flatten().collect();
            if !captures.contains("file") {
                let msg = format!(
                    "Regex for kind '{}' does not capture the required field `file`.",
                    key
                );
                return Err(serde::de::Error::custom(msg));
            }

            let kind_id = string_arena.insert(key);
            result.insert(Kind(kind_id), val);
        }
        Ok(Settings {
            string_arena: string_arena,
            inner: result,
            kinds_to_ignore: HashSet::new(), // Configured by command line
        })
    }
}

impl<'de> Deserialize<'de> for SettingsField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawSettings<'input> {
            #[serde(borrow)]
            regex: Cow<'input, str>,
            files: Vec<String>,
        }

        let raw = RawSettings::deserialize(deserializer)?;
        let as_regex: Regex = RegexBuilder::new(&raw.regex)
            .multi_line(true)
            .build()
            .map_err(serde::de::Error::custom)?;

        let categorizable = {
            let captures: HashSet<&str> = as_regex.capture_names().flatten().collect();
            captures.contains("category")
        };

        Ok(SettingsField {
            regex: as_regex,
            files: raw.files,
            categorizable: categorizable,
        })
    }
}

#[cfg(test)]
mod test {
    use toml;

    use super::*;

    #[test]
    fn can_deserialize_empty() {
        let settings_str = r#"
        "#;
        toml::from_str::<Settings>(settings_str).unwrap();
    }

    #[test]
    #[should_panic(expected = "missing field `regex`")]
    fn must_specify_regex() {
        let settings_str = r#"
        [gcc]
        files = ["**/*.txt"]
        "#;
        toml::from_str::<Settings>(settings_str).unwrap();
    }

    #[test]
    #[should_panic(expected = "missing field `files`")]
    fn must_specify_files() {
        let settings_str = r#"
        [gcc]
        regex = "warning: (?P<file>.+)"
        "#;
        toml::from_str::<Settings>(settings_str).unwrap();
    }

    #[test]
    #[should_panic(expected = "does not capture the required field `file`")]
    fn must_specify_file_capture_in_regex() {
        let settings_str = r#"
        [gcc]
        regex = "warning: (?P<description>.+)"
        files = ["**/*.txt"]
        "#;
        toml::from_str::<Settings>(settings_str).unwrap();
    }

    #[test]
    fn can_deserialize_many() {
        let settings_str = r#"
        [gcc]
        regex = "^(?P<file>[^:]+):(?P<line>\\d+):(?P<column>\\d+): warning: (?P<description>.+) \\[(?P<category>.+)\\]"
        files = ["**/gcc.txt"]

        [rust]
        regex = "^warning: (?P<description>.+)\n\\s+-->\\s(?P<file>[^:]+):(?P<line>\\d+):(?P<column>\\d+)$"
        files = ["**/rust.txt"]
        "#;

        let settings = toml::from_str::<Settings>(settings_str).unwrap();
        assert_eq!(settings.iter().count(), 2);
    }
}
