use std::collections::HashMap;
use std::fmt::Formatter;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

use crate::settings::Kind;
use std::fs::DirEntry;
use crate::utils::SearchableArena;
use config::ConfigError;

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub(crate) struct Category(String);

impl Category {
    pub fn none() -> Self {
        Category("_".to_owned())
    }

    pub fn from_str(s: &str) -> Self {
        Category(s.to_owned())
    }
}

#[derive(Debug)]
pub(crate) struct LimitsFile {
    inner: HashMap<Kind, LimitEntry>,
}


impl LimitsFile {
    pub fn iter(&self) -> impl Iterator<Item = (&Kind, &LimitEntry)> {
        self.inner.iter()
    }

    pub fn get(&self, key: &Kind) -> Option<&LimitEntry> {
        self.inner.get(key)
    }
}

#[derive(Debug)]
pub(crate) enum LimitEntry {
    Number(u64),
    PerCategory(HashMap<Category, u64>),
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


pub(crate) fn parse_limits_file(arena: &SearchableArena<String>, file: &Path) -> Result<LimitsFile, ConfigError> {
    let mut limits = config::Config::default();
    limits.merge(config::File::from(file))?;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawLimitEntry {
        Number(u64),
        PerCategory(HashMap<String, u64>),
    }

    let as_dict = limits.try_into::<HashMap<String, RawLimitEntry>>()?;
    let mut result = HashMap::new();

    for (key, val) in as_dict.into_iter() {
        let kind_id = arena.get(&key).unwrap_or_else(|| panic!("Have not seen this kind '{}' before!", key));
        let converted = match val {
            RawLimitEntry::Number(x) => LimitEntry::Number(x),
            RawLimitEntry::PerCategory(dict) => {
                LimitEntry::PerCategory(dict.iter().map(|(cat, x)| (Category::from_str(cat), *x)).collect())
            },
        };
        result.insert(Kind::new(kind_id), converted);
    }

    Ok(LimitsFile {
        inner: result,
    })
}
