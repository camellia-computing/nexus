mod privilege_broker_identity;

fn main() {
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        let target = std::env::var("TARGET").expect("Cargo did not provide TARGET");
        let extension = if target.contains("windows") {
            ".exe"
        } else {
            ""
        };
        let broker = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("Cargo did not provide manifest path"),
        )
        .join("binaries")
        .join(format!(
            "camellia-nexus-privilege-broker-{target}{extension}"
        ));
        println!("cargo:rerun-if-changed={}", broker.display());
        let broker_digest =
            privilege_broker_identity::digest_file_hex(&broker).unwrap_or_else(|error| {
            panic!(
                "privilege broker must be prepared and remain within the packaged size limit before building the desktop ({}): {error}",
                broker.display()
            )
        });
        println!("cargo:rustc-env=CAMELLIA_NEXUS_PRIVILEGE_BROKER_SHA256={broker_digest}");
        println!("cargo:rerun-if-changed=windows-app-manifest.xml");
        let windows = tauri_build::WindowsAttributes::new()
            .app_manifest(include_str!("windows-app-manifest.xml"));
        let attributes = tauri_build::Attributes::new().windows_attributes(windows);
        tauri_build::try_build(attributes).expect("failed to run Tauri build script");
    }
}
