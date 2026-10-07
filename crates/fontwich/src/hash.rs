//! Hash maps and sets, for tables nothing reads in order.
//!
//! hashbrown's, with a fixed hasher of our own rather than its default:
//! that one seeds itself from a process-wide static, and nothing here keeps
//! state outside what it is given. The keys are addresses, hashes fontwich
//! computed and paths the platform listed, not input anyone chooses to make
//! collide, so a fixed hash is safe and a fast one is enough: the Fx hash,
//! a rotate, an xor and a multiply per word.

use core::hash::{BuildHasher, Hasher};

/// A map with the [`Fx`] hash: a collection's fallback answers by key, and
/// the tables of files and of what a platform lists.
pub(crate) type HashMap<K, V> = hashbrown::HashMap<K, V, Build>;

/// A set with the [`Fx`] hash.
pub(crate) type HashSet<T> = hashbrown::HashSet<T, Build>;

/// Builds an [`Fx`], the same every time.
#[derive(Copy, Clone, Default, Debug)]
pub(crate) struct Build;

impl BuildHasher for Build {
    type Hasher = Fx;

    fn build_hasher(&self) -> Fx {
        Fx(0)
    }
}

/// The Fx hash, as rustc uses it: a word at a time, rotated in and
/// multiplied through.
pub(crate) struct Fx(u64);

/// The multiplier: odd, and with its bits spread, as Fx has it.
const K: u64 = 0xf135_7aea_2e62_a9c5;

impl Fx {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(K);
    }
}

impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for word in words {
            self.add(u64::from_le_bytes(*word));
        }
        if !rest.is_empty() {
            let mut last = [0u8; 8];
            last[..rest.len()].copy_from_slice(rest);
            // The length too, so that a shorter tail of zeros differs.
            self.add(u64::from_le_bytes(last) ^ ((rest.len() as u64) << 56));
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.add(u64::from(n));
    }

    fn write_u16(&mut self, n: u16) {
        self.add(u64::from(n));
    }

    fn write_u32(&mut self, n: u32) {
        self.add(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.add(n);
    }

    fn write_usize(&mut self, n: usize) {
        self.add(n as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_and_a_set_work_as_maps_and_sets_do() {
        let mut map: HashMap<&str, u32> = HashMap::default();
        for (at, name) in ["Arial", "Segoe UI", "Noto Sans", "arial"]
            .iter()
            .enumerate()
        {
            map.insert(name, at as u32);
        }
        assert_eq!(map.get("Arial"), Some(&0));
        assert_eq!(map.get("arial"), Some(&3));
        assert_eq!(map.len(), 4);
        let mut set: HashSet<usize> = HashSet::default();
        assert!(set.insert(0x1000));
        assert!(!set.insert(0x1000));
    }

    #[test]
    fn tails_of_different_lengths_hash_apart() {
        let hash = |bytes: &[u8]| {
            let mut fx = Build.build_hasher();
            fx.write(bytes);
            fx.finish()
        };
        assert_ne!(hash(b"a"), hash(b"a\0"));
        assert_ne!(hash(b""), hash(b"\0"));
        assert_eq!(hash(b"Segoe UI"), hash(b"Segoe UI"));
    }
}
