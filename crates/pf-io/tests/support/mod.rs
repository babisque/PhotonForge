use std::path::Path;

/// Compares `actual` with `tests/golden/<name>`. With `PF_UPDATE_GOLDEN` set,
/// rewrites the file instead; review the diff before committing.
pub fn assert_golden(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("PF_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    assert_eq!(expected, actual, "golden mismatch: {name}");
}
