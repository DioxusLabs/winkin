//! Small in-place sorts.
//!
//! [`by_key`] is not stable. It returns at once if the items are in order,
//! sorts a few by insertion and more by heapsort. [`stable_by_key`] is an
//! insertion sort that keeps equal items in order.
//!
//! The crate sorts a charset's pages as a cmap lists them, a few dozen at
//! most. `core`'s sorts cost some 4 KB of code for each type they sort.
//! These cost a few hundred bytes and allocate nothing.

/// How many items are few enough to sort by insertion, which is quickest
/// for so few, as `core`'s sort finds.
const FEW: usize = 20;

/// Sorts `items` by `key`, smallest first, in n log n steps however many
/// come in whatever order.
///
/// Not stable: items whose keys are equal come out in no order promised.
pub(crate) fn by_key<T, K: Ord>(items: &mut [T], mut key: impl FnMut(&T) -> K) {
    if items.is_sorted_by_key(&mut key) {
        return;
    }
    if items.len() <= FEW {
        stable_by_key(items, key);
        return;
    }
    // A heap of them all, the largest on top; then the top swapped to the
    // end, and the heap before it made whole again, until one is left.
    for node in (0..items.len() / 2).rev() {
        sift_down(items, node, &mut key);
    }
    for end in (1..items.len()).rev() {
        items.swap(0, end);
        if let Some(heap) = items.get_mut(..end) {
            sift_down(heap, 0, &mut key);
        }
    }
}

/// Sorts `items` by `key`, smallest first, keeping items with equal keys in
/// the order they came.
///
/// Moves each item back past those before it with a greater key: n² steps
/// at worst, and n when the items are nearly in order.
pub(crate) fn stable_by_key<T, K: Ord>(items: &mut [T], mut key: impl FnMut(&T) -> K) {
    for at in 1..items.len() {
        // Back past each item before it with a greater key.
        let mut to = at;
        while let Some(before) = to.checked_sub(1) {
            match (items.get(before), items.get(to)) {
                (Some(earlier), Some(item)) if key(earlier) > key(item) => {
                    items.swap(before, to);
                }
                _ => break,
            }
            to = before;
        }
    }
}

/// Moves the item at `node` of `heap` down until neither of the items below
/// it is larger.
fn sift_down<T, K: Ord>(heap: &mut [T], mut node: usize, key: &mut impl FnMut(&T) -> K) {
    let Some(sinking) = heap.get(node).map(&mut *key) else {
        return;
    };
    loop {
        let left = node.saturating_mul(2).saturating_add(1);
        let right = left.saturating_add(1);
        let (child, larger) = match (heap.get(left), heap.get(right)) {
            (Some(left_item), Some(right_item)) => {
                let (left_key, right_key) = (key(left_item), key(right_item));
                if left_key < right_key {
                    (right, right_key)
                } else {
                    (left, left_key)
                }
            }
            (Some(left_item), None) => (left, key(left_item)),
            (None, _) => return,
        };
        if sinking >= larger {
            return;
        }
        heap.swap(node, child);
        node = child;
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec::Vec;

    use super::{by_key, stable_by_key};

    /// Runs of every length up to 200, rising, falling, shuffled and
    /// shuffled with many keys the same.
    fn runs() -> Vec<Vec<(u32, usize)>> {
        let mut state = 0x2545_f491_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        let mut runs = Vec::new();
        for len in 0..200u32 {
            for shape in 0..4 {
                let run = (0..len)
                    .map(|at| match shape {
                        0 => at,
                        1 => len - at,
                        2 => next() % 16,
                        _ => next(),
                    })
                    .enumerate()
                    .map(|(position, key)| (key, position))
                    .collect();
                runs.push(run);
            }
        }
        runs
    }

    #[test]
    fn the_heapsort_sorts_as_core_does() {
        for run in runs() {
            let keys: Vec<u32> = run.iter().map(|&(key, _)| key).collect();
            let mut ours = keys.clone();
            by_key(&mut ours, |&key| key);
            let mut cores = keys;
            cores.sort_unstable();
            assert_eq!(ours, cores);
        }
    }

    #[test]
    fn the_insertion_sort_keeps_equal_keys_in_order_as_core_does() {
        for run in runs() {
            let mut ours = run.clone();
            stable_by_key(&mut ours, |&(key, _)| key);
            let mut cores = run;
            cores.sort_by_key(|&(key, _)| key);
            assert_eq!(ours, cores);
        }
    }
}
