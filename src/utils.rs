use id_arena::{Arena, Id};
use std::hash::{Hash, Hasher};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;

#[derive(Debug)]
pub(crate) struct SearchableArena<T: Hash> {
    arena: Arena<T>,
    mapping: HashMap<u64, Id<T>>,
}

impl<T: Hash> SearchableArena<T> {
    pub fn new() -> Self {
        SearchableArena {
            arena: Arena::new(),
            mapping: HashMap::new(),
        }
    }

    pub fn insert(&mut self, val: T) -> Id<T>{
        let mut s = DefaultHasher::new();
        val.hash(&mut s);
        let id = self.arena.alloc(val);
        self.mapping.insert(s.finish(), id);
        id
    }

    pub fn get(&self, val: &T) -> Option<Id<T>> {
        let mut s = DefaultHasher::new();
        val.hash(&mut s);
        self.mapping.get(&s.finish()).cloned()
    }
}
