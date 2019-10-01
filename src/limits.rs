use std::collections::HashMap;
use std::fmt::Formatter;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

use crate::settings::Kind;

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


impl<'de> Deserialize<'de> for LimitsFile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
    {
        let raw = <HashMap<Kind, LimitEntry>>::deserialize(deserializer)?;
        Ok(LimitsFile{ inner: raw })
    }
}


#[derive(Debug, Deserialize)]
#[serde(untagged)]
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
