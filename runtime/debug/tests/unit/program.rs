use super::same_imports;
use crate::engine::JsString;
use std::collections::HashSet;

#[test]
fn native_import_comparison_is_exact_set_equality() {
    let s = JsString::from_utf8;
    let expected: HashSet<_> = [s("a"), s("b")].into_iter().collect();
    assert!(same_imports(&[s("b"), s("a")], &expected));
    assert!(!same_imports(&[s("a"), s("a")], &expected));
    assert!(!same_imports(&[s("a")], &expected));
    assert!(!same_imports(&[s("a"), s("c")], &expected));
    assert!(!same_imports(&[s("a"), s("b"), s("b")], &expected));
}
