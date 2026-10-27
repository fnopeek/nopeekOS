//! A byte-bounded cache that evicts the least recently used entries, as
//! Linux's shrinkers give back the coldest objects first: a full cache
//! drops what has not been drawn for longest, not everything at once.

use hashbrown::HashMap;

struct Entry<V> {
    value: V,
    used: u64,
    size: usize,
}

pub struct LruBytes<K, V> {
    map: HashMap<K, Entry<V>>,
    bytes: usize,
    max: usize,
    clock: u64,
}

impl<K: Copy + Eq + core::hash::Hash, V> LruBytes<K, V> {
    pub fn new(max: usize) -> Self {
        LruBytes { map: HashMap::new(), bytes: 0, max, clock: 0 }
    }

    pub fn len(&self) -> usize { self.map.len() }

    pub fn contains_key(&self, k: &K) -> bool { self.map.contains_key(k) }

    /// The entry for `k`, marked as just used.
    pub fn get(&mut self, k: &K) -> Option<&V> {
        self.clock += 1;
        let now = self.clock;
        self.map.get_mut(k).map(|e| { e.used = now; &e.value })
    }

    /// Insert `value` of `size` bytes. When it does not fit, the coldest
    /// entries go until a quarter of the bound is free, so eviction is not
    /// paid on every insert.
    pub fn insert(&mut self, k: K, value: V, size: usize) {
        if self.bytes + size > self.max {
            let target = (self.max - self.max / 4).saturating_sub(size);
            let mut ages: alloc::vec::Vec<(u64, K)> =
                self.map.iter().map(|(k, e)| (e.used, *k)).collect();
            ages.sort_unstable_by_key(|a| a.0);
            for (_, key) in ages {
                if self.bytes <= target { break; }
                if let Some(old) = self.map.remove(&key) {
                    self.bytes = self.bytes.saturating_sub(old.size);
                }
            }
        }
        self.clock += 1;
        self.bytes += size;
        if let Some(old) = self.map.insert(k, Entry { value, used: self.clock, size }) {
            self.bytes = self.bytes.saturating_sub(old.size);
        }
    }
}
