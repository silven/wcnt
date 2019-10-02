use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use id_arena::{Arena, Id};

#[derive(Debug)]
pub(crate) struct SearchableArena {
    arena: Arena<String>,
    mapping: HashMap<u64, Id<String>>,
}

impl SearchableArena {
    pub fn new() -> Self {
        SearchableArena {
            arena: Arena::new(),
            mapping: HashMap::new(),
        }
    }

    fn iter(&self) -> impl Iterator<Item=(Id<String>, &String)> {
        self.arena.iter()
    }

    pub fn insert(&mut self, val: String) -> Id<String>{
        let mut s = DefaultHasher::new();
        val.hash(&mut s);
        let id = self.arena.alloc(val);
        self.mapping.insert(s.finish(), id);
        id
    }

    pub fn get_id(&self, val: &str) -> Option<Id<String>> {
        let mut s = DefaultHasher::new();
        val.hash(&mut s);
        self.mapping.get(&s.finish()).cloned()
    }

    pub fn lookup(&self, id: Id<String>) -> Option<&String> {
        self.arena.get(id)
    }

    pub fn get_or_insert(&mut self, val: &str) -> Id<String> {
        self.get_id(val)
            .unwrap_or_else(|| self.insert(val.to_owned()))
    }

    pub fn add_all(&mut self, other: &SearchableArena) {
        for (_key, val) in other.iter() {
            self.get_or_insert(val);
        }
    }
}
