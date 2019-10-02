use std::collections::{HashMap, HashSet};
use std::fmt::Display;

use id_arena::Id;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Deserializer};

use crate::utils::SearchableArena;

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
        Ok(MyRegex(as_regex))
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub(crate) struct Kind(pub(crate) Id<String>);

impl Kind {
    pub fn new(id: Id<String>) -> Self {
        Kind(id)
    }
}

#[derive(Debug)]
pub(crate) struct Settings {
    pub(crate) string_arena: SearchableArena,
    inner: HashMap<Kind, SettingsField>,
}

impl Settings {
    pub fn iter(&self) -> impl Iterator<Item = (&Kind, &SettingsField)> {
        self.inner.iter()
    }

    pub fn get(&self, key: &Kind) -> Option<&SettingsField> {
        self.inner.get(key)
    }

    pub fn display(&self) -> impl Display {
        use std::fmt::Write;

        let mut buff = String::new();
        writeln!(buff, "Settings {{");
        for (kind, field) in &self.inner {
            writeln!(buff, "[{}]", self.string_arena.lookup(kind.0).unwrap());
            writeln!(buff, "regex = {:?}", field.regex);
            writeln!(buff, "files = [{}]", field.files.join(", "));
            writeln!(buff, "default = {}", field.default.unwrap_or(0));

        }
        write!(buff, "}}");
        buff
    }
}

#[derive(Debug)]
pub(crate) struct SettingsField {
    pub(crate) regex: Regex,
    pub(crate) files: Vec<String>,
    command: Option<String>,
    pub(crate) default: Option<u64>,
}

impl<'de> Deserialize<'de> for Settings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = <HashMap<String, SettingsField>>::deserialize(deserializer)?;
        let mut result = HashMap::new();
        let mut string_arena = SearchableArena::new();
        for (key, val) in raw.into_iter() {

            let captures: HashSet<&str> = val.regex.capture_names().flatten().collect();
            if !captures.contains("file") {
                let msg = format!("Regex for kind '{}' does not capture the required field 'file'.", key);
                return Err(serde::de::Error::custom(msg));
            }

            let kind_id = string_arena.insert(key);
            result.insert(Kind(kind_id), val);
        }
        Ok(Settings { string_arena: string_arena, inner: result })
    }
}

impl<'de> Deserialize<'de> for SettingsField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawSettings {
            regex: String,
            files: Vec<String>,
            command: Option<String>,
            default: Option<u64>,
        }

        let raw = RawSettings::deserialize(deserializer)?;
        let as_regex: Regex = RegexBuilder::new(&raw.regex)
            .multi_line(true)
            .build()
            .map_err(serde::de::Error::custom)?;

        let names: HashSet<&str> = as_regex.capture_names().filter_map(|n| n).collect();
        if !names.contains("file") {
            let msg = format!(
                "Regex '{}' does not contain the required capture group 'file'.",
                as_regex
            );
            return Err(serde::de::Error::custom(msg));
        }
        Ok(SettingsField {
            regex: as_regex,
            files: raw.files,
            command: raw.command,
            default: raw.default,
        })
    }
}

