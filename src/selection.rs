//! Key matching for page replacements. Pages choose their own missing-target fallback.

/// The place in `new` of `old[index]`, matched by `key`. `None` when it is gone.
pub fn matching<T, K: PartialEq>(old: &[T], index: usize, new: &[T], key: impl Fn(&T) -> K) -> Option<usize> {
    let wanted = key(old.get(index)?);
    new.iter().position(|item| key(item) == wanted)
}

/// Keep the target, or its numeric position clamped to the new list. Empty lists use the inactive index zero.
pub fn kept_index<T, K: PartialEq>(old: &[T], index: usize, new: &[T], key: impl Fn(&T) -> K) -> usize {
    matching(old, index, new, key).unwrap_or_else(|| index.min(new.len().saturating_sub(1)))
}

#[cfg(test)]
mod tests {
    use super::{kept_index, matching};

    #[test]
    fn targets_follow_keys_and_missing_targets_use_the_pages_fallback() {
        let old = ["a", "b", "c", "d"];
        let key = |s: &&str| s.to_string();
        assert_eq!(matching(&old, 2, &["c", "b", "a"], key), Some(0));
        assert_eq!(matching(&old, 2, &["a", "b"], key), None);
        assert_eq!(matching(&old, 8, &["a"], key), None);
        assert_eq!(kept_index(&old, 2, &["new", "a", "b", "c", "d"], key), 3);
        assert_eq!(kept_index(&old, 2, &["a", "b", "d", "e"], key), 2);
        assert_eq!(kept_index(&old, 3, &["a", "b"], key), 1);
        assert_eq!(kept_index(&old, 1, &[], key), 0);
    }
}
