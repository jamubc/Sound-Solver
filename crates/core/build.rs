//! Hashes the solver's source (`src`, `manifests`) into `EXHAUST_CORE_SOURCE`, so a result
//! cached by one build of the solver is never taken for another's.

use std::path::Path;

fn add(dir: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("source directory") {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            add(&path, files);
        } else {
            files.push(path);
        }
    }
}

fn main() {
    let mut files = Vec::new();
    for dir in ["src", "manifests"] {
        println!("cargo:rerun-if-changed={dir}");
        add(Path::new(dir), &mut files);
    }
    files.sort();
    let mut hasher = blake3::Hasher::new();
    for f in files {
        hasher.update(f.to_string_lossy().as_bytes());
        hasher.update(&std::fs::read(&f).expect("source file"));
    }
    println!(
        "cargo:rustc-env=EXHAUST_CORE_SOURCE={}",
        &hasher.finalize().to_hex()[..16]
    );
}
