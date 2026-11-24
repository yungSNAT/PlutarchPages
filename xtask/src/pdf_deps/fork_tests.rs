use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{PACKAGES, SHARED_REV, VENDORED, validate};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// What `cargo metadata` reports for this fork: the vendored packages from their local copies,
/// the other Hayro packages from craft-libs at the shared revision, lopdf from its vendor copy.
fn metadata(root: &Path, local: &[&str]) -> Value {
    let git = format!("git+https://github.com/storytold/craft-libs?rev={SHARED_REV}#{SHARED_REV}");
    let mut packages: Vec<Value> = PACKAGES
        .iter()
        .map(|&(name, version)| {
            let manifest = root.join("vendor").join(name).join("Cargo.toml");
            let source = if local.contains(&name) { Value::Null } else { json!(git) };
            json!({ "name": name, "version": version, "id": name, "source": source, "manifest_path": manifest })
        })
        .collect();
    packages.push(json!({ "name": "lopdf", "version": "0.45.0", "id": "lopdf", "source": null, "manifest_path": root.join("vendor/lopdf/Cargo.toml") }));
    json!({ "packages": packages, "resolve": { "nodes": [{ "id": "craft-pdf", "features": ["read", "interpret", "raster"] }] } })
}

#[test]
fn the_vendored_craft_pdf_and_hayro_interpret_pass() {
    let root = root();
    assert_eq!(validate(&root, &metadata(&root, VENDORED)), Ok(()));
}

#[test]
fn a_vendored_package_resolved_from_git_fails() {
    // Building craft-libs' own craft-pdf or hayro-interpret would drop the fork's patches.
    let root = root();
    for name in VENDORED {
        let local: Vec<&str> = VENDORED.iter().copied().filter(|n| n != name).collect();
        let err = validate(&root, &metadata(&root, &local)).unwrap_err();
        assert!(err.starts_with(&format!("{name}: expected")), "{err}");
    }
}

#[test]
fn another_hayro_package_from_a_local_copy_fails() {
    let root = root();
    let mut local = VENDORED.to_vec();
    local.push("hayro-syntax");
    let err = validate(&root, &metadata(&root, &local)).unwrap_err();
    assert!(err.starts_with("hayro-syntax: expected"), "{err}");
}
