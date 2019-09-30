use std::collections::{HashMap, HashSet};
use std::fmt::Formatter;
use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Deserializer};

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

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub(crate) struct Kind(String);

impl<'de> Deserialize<'de> for Kind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Kind(raw))
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub(crate) struct Category(String);

impl<'de> Deserialize<'de> for Category {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Category(raw))
    }
}


impl Category {
    pub fn none() -> Self {
        Category("_".to_owned())
    }

    pub fn from_str(s: &str) -> Self {
        Category(s.to_owned())
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum LimitEntry {
    Number(u64),
    PerCategory(HashMap<Category, u64>),
}

#[derive(Debug)]
pub(crate) struct Settings {
    inner: HashMap<Kind, SettingsField>,
}

impl Settings {
    pub fn iter(&self) -> impl Iterator<Item = (&Kind, &SettingsField)> {
        self.inner.iter()
    }

    pub fn get(&self, key: &Kind) -> Option<&SettingsField> {
        self.inner.get(key)
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
        for (key, val) in raw.into_iter() {
            result.insert(Kind(key), val);
        }

        Ok(Settings { inner: result })
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

#[derive(PartialEq, Eq, Hash)]
pub(crate) struct LimitsEntry {
    pub(crate) limits_file: Option<PathBuf>,
    pub(crate) kind: Kind,
    pub(crate) category: Category,
}

impl LimitsEntry {
    pub fn new<T: AsRef<Path>>(
        limits_file: Option<T>,
        kind: &Kind,
        category: Option<Category>,
    ) -> Self {
        LimitsEntry {
            limits_file: limits_file.map(|x| PathBuf::from(x.as_ref())),
            kind: kind.clone(),
            category: category.unwrap_or_else(Category::none),
        }
    }

    pub fn without_category(&self) -> Self {
        LimitsEntry {
            limits_file: self.limits_file.clone(),
            kind: self.kind.clone(),
            category: Category("_".to_owned()),
        }
    }
}

impl core::fmt::Debug for LimitsEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), std::fmt::Error> {
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
        write!(f, ":[{:?}/{:?}]", self.kind, self.category)
    }
}
