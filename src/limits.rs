//! Module responsible for structures and functionality related to Limits and Limit files.
use std::collections::HashSet;
use std::error::Error;
use std::fmt::Display;
use std::fs::read_to_string;
use std::path::{Path, PathBuf};

use id_arena::Id;
use linked_hash_map::LinkedHashMap;
use serde::{Deserialize, Serialize};
use toml;

use crate::settings::Kind;
use crate::utils;
use crate::utils::SearchableArena;
use crate::warnings::EntryCount;

#[derive(Debug, PartialEq, Eq, Ord, PartialOrd, Hash, Clone)]
/// A Category represents a specific type of warning, hierarchically below a [Kind](../settings/struct.Kind.html).
/// Examples of Categories could be [-Wunsued-value](https://gcc.gnu.org/onlinedocs/gcc/Warning-Options.html),
/// or [F401](https://flake8.pycqa.org/en/latest/user/error-codes.html).
/// The "_" category is the "wildcard" category. It matches all previously undeclared categories.
pub(crate) struct Category(Option<Id<String>>);

impl Category {
    pub fn new(id: Id<String>) -> Self {
        Category(Some(id))
    }

    pub fn none() -> Self {
        Category(None)
    }

    pub fn remap_id(&mut self, from: &SearchableArena, to: &SearchableArena) {
        if let Some(cat_id) = self.0 {
            let cat_str = from
                .lookup(cat_id)
                .expect("String not present in new arena. Did you forget to call add_all?");
            self.0 = to.get_id(cat_str);
        }
    }

    pub fn to_str<'arena>(&self, arena: &'arena SearchableArena) -> Option<&'arena str> {
        match self.0 {
            Some(cat_id) => Some(arena.lookup(cat_id).unwrap()),
            None => None,
        }
    }

    fn from_str(as_str: &str, arena: &mut SearchableArena) -> Self {
        if as_str == "_" {
            Category::none()
        } else {
            Category::new(arena.get_or_insert(as_str))
        }
    }
}

/// A LimitsFile declares a [Limit](struct.Limit.html) for a [Kind](../settings/struct.Kind.html) as a threshold
/// of number of warnings allowed.
#[derive(Clone, PartialEq)]
pub(crate) struct LimitsFile {
    inner: LinkedHashMap<Kind, Limit>,
}

impl LimitsFile {
    pub fn iter(&self) -> impl Iterator<Item = (&Kind, &Limit)> {
        self.inner.iter()
    }

    #[cfg(test)]
    pub fn get_limit(&self, kind: &Kind) -> Option<&Limit> {
        self.inner.get(kind)
    }

    pub(crate) fn as_serializable(&self, arena: &SearchableArena) -> impl Serialize {
        #[derive(Serialize)]
        #[serde(untagged)]
        enum IntOrFloat {
            I(u64),
            F(f32),
        }

        #[derive(Serialize)]
        #[serde(untagged)]
        enum RawLimitEntry {
            Number(IntOrFloat),
            PerCategory(LinkedHashMap<String, IntOrFloat>),
        }

        #[derive(Serialize)]
        #[serde(untagged)]
        enum Inner {
            #[serde(serialize_with = "toml::ser::tables_last")]
            V(LinkedHashMap<String, RawLimitEntry>),
        }

        let mut as_map = LinkedHashMap::new();
        for (kind, val) in &self.inner {
            let raw_val = match val {
                Limit::Number(Some(x)) => RawLimitEntry::Number(IntOrFloat::I(*x)),
                Limit::Number(None) => RawLimitEntry::Number(IntOrFloat::F(std::f32::INFINITY)),
                Limit::PerCategory(dict) => {
                    let mut cat_dict = LinkedHashMap::new();
                    for (cat, val) in dict {
                        let limit = match val {
                            Some(x) => IntOrFloat::I(*x),
                            None => IntOrFloat::F(std::f32::INFINITY),
                        };
                        cat_dict.insert(cat.to_str(&arena).unwrap_or("_").to_owned(), limit);
                    }
                    RawLimitEntry::PerCategory(cat_dict)
                }
            };
            as_map.insert(kind.to_str(&arena).to_owned(), raw_val);
        }
        Inner::V(as_map)
    }

    pub fn display<'me, 'arena: 'me>(
        &'me self,
        arena: &'arena SearchableArena,
    ) -> impl Display + 'me {
        utils::fmt_helper(move |f| {
            let as_string = toml::ser::to_string(&self.as_serializable(&arena)).map_err(|e| {
                eprintln!("Could not display LimitsFile: `{}`", e);
                std::fmt::Error
            })?;
            write!(f, "{}", as_string)
        })
    }

    pub fn zero(&mut self, these: &HashSet<&Kind>) {
        for (kind, limit) in self.inner.iter_mut() {
            if !these.contains(&kind) {
                continue;
            }
            match limit {
                Limit::Number(Some(x)) => *x = 0,
                Limit::Number(None) => { /* inf limit, do nothing */ }
                Limit::PerCategory(per_cat) => {
                    for (_cat, inner_limit) in per_cat.iter_mut() {
                        match inner_limit {
                            Some(x) => *x = 0,
                            None => { /* inf limit, do nothing*/ }
                        }
                    }
                }
            }
        }
    }

    pub fn prune_categories(&mut self) {
        enum PruneResult<'a> {
            AllZero,
            OnlyOne(&'a Option<u64>),
            StillSomeLeft,
        }

        for (_kind, limit) in self.inner.iter_mut() {
            let prune_result = if let Limit::PerCategory(per_cat) = limit {
                // LinkedHashMap doesn't have retain() :'(
                *per_cat = per_cat
                    .into_iter()
                    // Checks if the contained value is zero, Option::contains is not stable yet.
                    .filter(|(_cat, val)| !(val.is_some() && val.unwrap() == 0))
                    .map(|(cat, val)| (cat.clone(), val.clone()))
                    .collect::<LinkedHashMap<Category, Option<u64>>>();
                if per_cat.is_empty() {
                    PruneResult::AllZero
                } else if per_cat.len() == 1 {
                    PruneResult::OnlyOne(per_cat.values().next().unwrap())
                } else {
                    PruneResult::StillSomeLeft
                }
            } else {
                PruneResult::StillSomeLeft
            };

            match prune_result {
                PruneResult::AllZero => {
                    *limit = Limit::Number(Some(0));
                }
                PruneResult::OnlyOne(value) => *limit = Limit::Number(value.clone()),
                PruneResult::StillSomeLeft => { /* do nothing */ }
            }
        }
    }

    pub fn update_limits(&mut self, updated_count: &EntryCount) {
        let limit = self
            .inner
            .get_mut(&updated_count.entry().kind)
            .expect("Kind not found in LimitsFile!");
        let actual = updated_count.actual;
        match limit {
            Limit::Number(Some(x)) => *x = actual,
            Limit::Number(None) => { /* inf limit, do nothing */ }
            Limit::PerCategory(per_cat) => {
                let inner_limit = per_cat.get_mut(&updated_count.entry().category);
                if let Some(maybe_limit) = inner_limit {
                    match maybe_limit {
                        Some(x) => *x = actual,
                        None => { /* inf limit, do nothing*/ }
                    }
                } else {
                    panic!("We got a warning for a category that we don't have?");
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
/// A Limit can either be a single number, which should hold for any [Category](struct.Category.html)
/// of warnings for that [Kind](../settings/struct.Kind.html), or be declared per category.
/// A limit may also be "infinity", represented by None.
pub(crate) enum Limit {
    Number(Option<u64>),
    PerCategory(LinkedHashMap<Category, Option<u64>>),
}

#[derive(PartialEq, Eq, Ord, PartialOrd, Debug, Clone, Hash)]
/// A LimitsEntry is a shorthand representation for a single numerical threshold within the system.
/// To uniquely identify a [Limit](enum.Limit.html), you need a Path, a
/// [Kind](../settings/struct.Kind.html) and a [Category](struct.Category.html).
pub(crate) struct LimitsEntry {
    pub(crate) limits_file: Option<PathBuf>,
    pub(crate) kind: Kind,
    pub(crate) category: Category,
}

impl LimitsEntry {
    pub fn new(limits_file: Option<&Path>, kind: Kind, category: Category) -> Self {
        LimitsEntry {
            limits_file: limits_file.map(PathBuf::from),
            kind: kind,
            category: category,
        }
    }

    pub fn without_category(&self) -> Self {
        LimitsEntry {
            limits_file: self.limits_file.clone(),
            kind: self.kind.clone(),
            category: Category::none(),
        }
    }

    pub fn display<'me, 'arena: 'me>(
        &'me self,
        arena: &'arena SearchableArena,
    ) -> impl Display + 'me {
        utils::fmt_helper(move |f| {
            match self.limits_file {
                Some(ref pb) => {
                    let tmp: PathBuf; // Sometimes I think things are a little silly
                    let path = if pb.components().count() > 5 {
                        // Silly way to take the last 4 components of the path
                        tmp = PathBuf::from("...").join(
                            pb.components()
                                .rev()
                                .take(4)
                                .collect::<PathBuf>()
                                .components()
                                .rev()
                                .collect::<PathBuf>(),
                        );
                        &tmp
                    } else {
                        pb
                    };
                    write!(f, "{}", path.display())?;
                }
                None => {
                    write!(f, "_")?;
                }
            };
            write!(
                f,
                ":[{}/{}]",
                self.kind.to_str(&arena),
                self.category.to_str(&arena).unwrap_or("_")
            )
        })
    }
}

/// Parse a `Limits.toml` file into a [LimitsFile](struct.LimitsFile.html) structure.
pub(crate) fn parse_limits_file(
    arena: &mut SearchableArena,
    file: &Path,
    categorizables: &HashSet<Kind>,
) -> Result<LimitsFile, Box<dyn Error>> {
    let file_contents = read_to_string(file)?;
    parse_limits_file_from_str(arena, &file_contents, &categorizables)
        .map_err(|e| format!("Could not parse `{}`. Reason `{}`", file.display(), e).into())
}

/// Parse the string `cfg` in toml format into a [LimitsFile](struct.LimitsFile.html) structure.
fn parse_limits_file_from_str(
    arena: &mut SearchableArena,
    cfg: &str,
    categorizables: &HashSet<Kind>,
) -> Result<LimitsFile, Box<dyn Error>> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum IntOrFloat {
        I(u64),
        F(f32),
    }

    impl IntOrFloat {
        fn to_limit(&self) -> Result<Option<u64>, Box<dyn Error>> {
            match *self {
                IntOrFloat::I(i) => Ok(Some(i)),
                IntOrFloat::F(f) => {
                    if f.is_sign_positive() && f.is_infinite() {
                        Ok(None)
                    } else {
                        Err("Limit values can only be a positive integer or `inf`.".into())
                    }
                }
            }
        }
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum RawLimitEntry<'input> {
        Number(IntOrFloat),
        #[serde(borrow)]
        PerCategory(LinkedHashMap<&'input str, IntOrFloat>),
    }

    let as_raw_dict: LinkedHashMap<&str, RawLimitEntry> = toml::from_str(&cfg)?;
    let mut result = LinkedHashMap::new();

    for (key, val) in as_raw_dict.into_iter() {
        let kind_id = arena.get_id(&key).ok_or_else(|| {
            format!(
                "Referred to kind `{}` which has not been configured in the settings.",
                key
            )
        })?;
        let kind = Kind::new(kind_id);
        let converted = match val {
            RawLimitEntry::Number(x) => Limit::Number(x.to_limit()?),
            RawLimitEntry::PerCategory(dict) => {
                if !categorizables.contains(&kind) {
                    return Err(format!("Kind `{}` is not categorizable.", key).into());
                }
                let mut per_category = LinkedHashMap::new();
                for (cat_str, x) in dict {
                    let limit = x.to_limit()?;
                    let category = Category::from_str(cat_str, arena);
                    per_category.insert(category, limit);
                }

                Limit::PerCategory(per_category)
            }
        };
        result.insert(kind, converted);
    }

    Ok(LimitsFile { inner: result })
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn can_deserialize_empty() {
        let limits_str = r#"
        "#;

        let categorizable = HashSet::new();
        let mut arena = SearchableArena::new();
        parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).unwrap();
    }

    #[test]
    #[should_panic(expected = "kind `gcc`")]
    fn cannot_deserialize_with_unknown_kind() {
        let limits_str = r#"
        gcc = 1
        "#;

        let categorizable = HashSet::new();
        let mut arena = SearchableArena::new();
        parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).unwrap();
    }

    #[test]
    fn can_deserialize_with_known_kind() {
        let limits_str = r#"
        gcc = 1
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let categorizable = HashSet::new();
        let limits = parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).unwrap();

        assert_eq!(limits.get_limit(&gcc_kind), Some(&Limit::Number(Some(1))));
    }

    #[test]
    fn can_deserialize_with_categories() {
        let limits_str = r#"
        [gcc]
        -Wbad-code = 1
        -Wpedantic = 2
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let mut categorizable = HashSet::new();
        categorizable.insert(gcc_kind.clone());
        let limits =
            parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).expect("parse");

        let cat_bad_code = Category::new(arena.get_id("-Wbad-code").expect("bad code"));
        let cat_pedantic = Category::new(arena.get_id("-Wpedantic").expect("pedantic"));
        let expected_mapping: LinkedHashMap<Category, Option<u64>> =
            vec![(cat_bad_code, Some(1)), (cat_pedantic, Some(2))]
                .into_iter()
                .collect();
        assert_eq!(
            limits.get_limit(&gcc_kind),
            Some(&Limit::PerCategory(expected_mapping))
        );
    }

    #[test]
    #[should_panic(expected = "`gcc` is not categorizable")]
    fn cannot_deserialize_without_being_categorizable() {
        let limits_str = r#"
        [gcc]
        -Wbad-code = 1
        -Wpedantic = 2
        "#;

        let mut arena = SearchableArena::new();
        arena.insert("gcc".to_owned());
        let categorizable = HashSet::new();
        parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).expect("parse");
    }

    #[test]
    fn can_deserialize_with_wildcard() {
        let limits_str = r#"
        [gcc]
        -Wbad-code = 2
        _ = 1
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let mut categorizable = HashSet::new();
        categorizable.insert(gcc_kind.clone());
        let limits =
            parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).expect("parse");

        let cat_bad_code = Category::new(arena.get_id("-Wbad-code").expect("bad code"));
        let expected_mapping: LinkedHashMap<Category, Option<u64>> =
            vec![(cat_bad_code, Some(2)), (Category::none(), Some(1))]
                .into_iter()
                .collect();
        assert_eq!(
            limits.get_limit(&gcc_kind),
            Some(&Limit::PerCategory(expected_mapping))
        );
    }

    #[test]
    fn can_deserialize_inf() {
        let limits_str = r#"
        [gcc]
        -Wbad-code = inf
        _ = 1
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let mut categorizable = HashSet::new();
        categorizable.insert(gcc_kind.clone());
        let limits =
            parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).expect("parse");

        let cat_bad_code = Category::new(arena.get_id("-Wbad-code").expect("bad code"));
        let expected_mapping: LinkedHashMap<Category, Option<u64>> =
            vec![(cat_bad_code, None), (Category::none(), Some(1))]
                .into_iter()
                .collect();
        assert_eq!(
            limits.get_limit(&gcc_kind),
            Some(&Limit::PerCategory(expected_mapping))
        );
    }

    #[test]
    #[should_panic(expected = "only be a positive integer or `inf`")]
    fn wont_deserialize_floats() {
        let limits_str = r#"
        [gcc]
        -Wbad-code = 2.0
        _ = 1
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let mut categorizable = HashSet::new();
        categorizable.insert(gcc_kind.clone());
        let limits =
            parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).expect("parse");

        let cat_bad_code = Category::new(arena.get_id("-Wbad-code").expect("bad code"));
        let expected_mapping: LinkedHashMap<Category, Option<u64>> =
            vec![(cat_bad_code, Some(2)), (Category::none(), Some(1))]
                .into_iter()
                .collect();
        assert_eq!(
            limits.get_limit(&gcc_kind),
            Some(&Limit::PerCategory(expected_mapping))
        );
    }

    #[test]
    fn prune_only_one_into_simple_limit() {
        let limits_str = r#"
        [gcc]
        stuff = 0
        _ = 1
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let mut categorizable = HashSet::new();
        categorizable.insert(gcc_kind.clone());
        let mut limits =
            parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).expect("parse");

        limits.prune_categories();

        assert_eq!(
            "gcc = 1\n",
            toml::ser::to_string(&limits.as_serializable(&arena)).expect("Deserialize")
        );
    }

    #[test]
    fn prune_remove_zero_categories() {
        let limits_str = r#"
        [gcc]
        stuff = 1
        more-stuff = 0
        _ = 1
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let mut categorizable = HashSet::new();
        categorizable.insert(gcc_kind.clone());
        let mut limits =
            parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).expect("parse");

        limits.prune_categories();

        assert_eq!(
            r#"[gcc]
stuff = 1
_ = 1
"#,
            toml::ser::to_string(&limits.as_serializable(&arena)).expect("Deserialize")
        );
    }

    #[test]
    fn prune_turn_all_zero_into_simple() {
        let limits_str = r#"
        [gcc]
        stuff = 0
        more-stuff = 0
        "#;

        let mut arena = SearchableArena::new();
        let gcc_kind = Kind::new(arena.insert("gcc".to_owned()));
        let mut categorizable = HashSet::new();
        categorizable.insert(gcc_kind.clone());
        let mut limits =
            parse_limits_file_from_str(&mut arena, &limits_str, &categorizable).expect("parse");

        limits.prune_categories();

        assert_eq!(
            r#"gcc = 0
"#,
            toml::ser::to_string(&limits.as_serializable(&arena)).expect("Deserialize")
        );
    }
}
