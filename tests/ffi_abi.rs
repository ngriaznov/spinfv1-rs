//! Header ↔ implementation consistency for the C ABI.
//!
//! `tests/ffi.rs` exercises behavior through the Rust items, and
//! `tests/c/run.sh` compiles a real C harness against
//! `include/spinfv1.h` and the built static library (proving actual
//! C-linkage of every symbol, under sanitizers). This file closes the
//! remaining drift window between those two: it parses the
//! `#[unsafe(no_mangle)]` exports out of `src/ffi.rs` and the function
//! declarations out of `include/spinfv1.h`, and requires the two sets
//! to be identical — a function added, renamed, or removed on either
//! side fails here immediately, without needing a C compiler.
#![cfg(feature = "ffi")]

use std::collections::BTreeSet;
use std::path::Path;

fn repo(path: &str) -> String {
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// `spinfv1_*` identifiers immediately followed by `(` — i.e. actual
/// declarations/definitions/calls, not prose mentions.
fn called_names(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut names = BTreeSet::new();
    let mut i = 0;
    while let Some(pos) = text[i..].find("spinfv1_") {
        let start = i + pos;
        let end = start
            + text[start..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .map(char::len_utf8)
                .sum::<usize>();
        // Skip when embedded in a longer identifier (e.g. `libspinfv1`).
        let standalone =
            start == 0 || !(bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_');
        if standalone && bytes.get(end) == Some(&b'(') {
            names.insert(text[start..end].to_string());
        }
        i = end.max(start + 1);
    }
    names
}

/// Function names carrying `#[unsafe(no_mangle)]` in the FFI module.
fn exported_names(source: &str) -> BTreeSet<String> {
    source
        .split("#[unsafe(no_mangle)]")
        .skip(1)
        .filter_map(|after| {
            let fn_pos = after.find("fn ")?;
            let name: String = after[fn_pos + 3..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

#[test]
fn header_and_exports_match_exactly() {
    let exported = exported_names(&repo("src/ffi.rs"));
    let declared = called_names(&repo("include/spinfv1.h"));
    assert!(!exported.is_empty(), "no exports found — parser broken?");
    assert_eq!(
        declared, exported,
        "include/spinfv1.h and src/ffi.rs disagree about the exported ABI"
    );
}

#[test]
fn c_safety_harness_covers_every_symbol() {
    // The sanitizer battery (tests/c/run.sh) is the linkage proof, so
    // it must actually reference every exported function.
    let exported = exported_names(&repo("src/ffi.rs"));
    let used = called_names(&repo("tests/c/ffi_safety.c"));
    let missing: Vec<_> = exported.difference(&used).collect();
    assert!(
        missing.is_empty(),
        "tests/c/ffi_safety.c never calls: {missing:?}"
    );
}
