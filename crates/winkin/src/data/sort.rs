//! The crate's in-place sort: an insertion sort for a few items and a
//! heapsort for more.
//!
//! It returns at once where the items are in order already. The crate sorts
//! a line's shifted boxes, the segments of a line that bidi reorders, and the
//! styles of a paragraph's soft hyphens. These come a few at a time. Each
//! key is unique, or items with equal keys are the same, so any sort gives
//! the order `core`'s would.
//!
//! `core`'s sort is about 4 KB of code for each type it sorts. This one is a
//! few hundred bytes and allocates nothing. It is as quick for a few items,
//! and takes n log n steps for any input. `core`'s is two or three times
//! quicker on hundreds of items. The bidi algorithm sorts its bracket pairs
//! with `core`'s, because its conformance tests build it alone, outside the
//! crate.

/// How many items are few enough to sort by insertion, which is quickest
/// for so few, as `core`'s sort finds.
const FEW: usize = 20;

/// Sorts `items` by `key`, smallest first.
///
/// Not stable: items whose keys are equal come out in no order promised.
pub(crate) fn sort_by_key<T, K: Ord>(items: &mut [T], mut key: impl FnMut(&T) -> K) {
    if items.is_sorted_by_key(&mut key) {
        return;
    }
    if items.len() <= FEW {
        insert_each(items, &mut key);
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

/// Sorts `items` by `key` by moving each back past those before it with a
/// greater key.
fn insert_each<T, K: Ord>(items: &mut [T], key: &mut impl FnMut(&T) -> K) {
    for at in 1..items.len() {
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
