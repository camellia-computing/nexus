use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn collect_sources(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read Core source directory") {
        let entry = entry.expect("read Core source entry");
        let kind = entry.file_type().expect("read Core source type");
        if kind.is_dir() {
            collect_sources(&entry.path(), files);
        } else if kind.is_file() && entry.path().extension().is_some_and(|ext| ext == "rs") {
            files.push(entry.path());
        }
    }
}

fn main() {
    let mut files = vec![PathBuf::from("build.rs"), PathBuf::from("Cargo.toml")];
    collect_sources(Path::new("src"), &mut files);
    files.sort();
    let mut hash = Sha256::new();
    hash.update(b"nexus-core-implementation\0");
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        let name = path.to_string_lossy().replace('\\', "/");
        let bytes = fs::read(&path).expect("read Core implementation");
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(&bytes);
    }
    println!("cargo:rerun-if-changed=src");
    let revision: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    println!("cargo:rustc-env=NEXUS_CORE_IMPLEMENTATION_SHA256={revision}");
}
