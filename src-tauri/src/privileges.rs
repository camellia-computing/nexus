use std::{fs::File, io::Read, path::Path};

#[cfg(feature = "desktop")]
use camellia_nexus_core::ProgramKind;
use camellia_nexus_core::{
    AdapterRegistry, CamelliaNexusError, ErrorCode, LaunchPlan, PrivilegeAssessment,
    PrivilegeAssessmentContext, PrivilegeConfigInput, PrivilegePolicy, PrivilegeReason,
    PrivilegeRequirement, Result,
};

const ASSESSMENT_INPUT_LIMIT: u64 = 4 * 1024 * 1024;

pub(crate) fn assess_launch_plan(plan: &LaunchPlan) -> Result<PrivilegeAssessment> {
    let detection = detect_requirement(plan);
    let (detected, mut reasons, authoritative) = match detection {
        Ok(detection) => detection,
        Err(_) if !matches!(plan.privilege_policy, PrivilegePolicy::Automatic) => (
            PrivilegeRequirement::Unknown,
            vec![PrivilegeReason::ConfigurationUnavailable],
            false,
        ),
        Err(error) => return Err(error),
    };
    let effective = match plan.privilege_policy {
        PrivilegePolicy::Standard => PrivilegeRequirement::Standard,
        PrivilegePolicy::Elevated => {
            if !reasons.contains(&PrivilegeReason::ExplicitPolicy) {
                reasons.push(PrivilegeReason::ExplicitPolicy);
            }
            PrivilegeRequirement::Elevated
        }
        PrivilegePolicy::Automatic => match detected {
            PrivilegeRequirement::Elevated => PrivilegeRequirement::Elevated,
            PrivilegeRequirement::Standard | PrivilegeRequirement::Unknown => {
                PrivilegeRequirement::Standard
            }
        },
    };
    Ok(PrivilegeAssessment {
        detected,
        effective,
        reasons,
        authoritative,
    })
}

fn detect_requirement(
    plan: &LaunchPlan,
) -> Result<(PrivilegeRequirement, Vec<PrivilegeReason>, bool)> {
    #[cfg(windows)]
    if windows_manifest_requests_elevation(&plan.executable)? {
        return Ok((
            PrivilegeRequirement::Elevated,
            vec![PrivilegeReason::ExecutableManifest],
            true,
        ));
    }

    if plan.privilege_inputs.is_empty() {
        return Ok((
            PrivilegeRequirement::Unknown,
            vec![PrivilegeReason::ConfigurationUnavailable],
            false,
        ));
    }
    let adapter = AdapterRegistry::default().get(plan.program_kind);
    let documents = read_config_inputs(&plan.privilege_inputs)?;
    if documents.is_empty() {
        return Ok((
            PrivilegeRequirement::Unknown,
            vec![PrivilegeReason::ConfigurationUnavailable],
            false,
        ));
    }
    let mut reasons = Vec::new();
    for bytes in documents {
        let Some(document_reasons) = adapter.assess_privilege_configuration(
            &bytes,
            PrivilegeAssessmentContext::for_current_platform(),
        )?
        else {
            return Ok((
                PrivilegeRequirement::Unknown,
                vec![PrivilegeReason::ConfigurationUnavailable],
                false,
            ));
        };
        reasons.extend(document_reasons);
    }
    reasons = camellia_nexus_core::normalize_reasons(reasons);
    let requirement = if reasons.is_empty() {
        PrivilegeRequirement::Standard
    } else {
        PrivilegeRequirement::Elevated
    };
    Ok((requirement, reasons, true))
}

fn read_config_inputs(inputs: &[PrivilegeConfigInput]) -> Result<Vec<Vec<u8>>> {
    let mut paths = std::collections::BTreeSet::new();
    for input in inputs {
        match input {
            PrivilegeConfigInput::File { path } => {
                paths.insert(path.clone());
            }
            PrivilegeConfigInput::Directory { path } => {
                let metadata =
                    std::fs::symlink_metadata(path).map_err(CamelliaNexusError::storage)?;
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::PrivilegeConfigUnsafe,
                        "Privilege assessment requires a regular configuration directory",
                    ));
                }
                for entry in std::fs::read_dir(path).map_err(CamelliaNexusError::storage)? {
                    let entry = entry.map_err(CamelliaNexusError::storage)?;
                    let entry_path = entry.path();
                    if entry_path
                        .extension()
                        .and_then(std::ffi::OsStr::to_str)
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
                    {
                        paths.insert(entry_path);
                    }
                }
            }
        }
    }
    let mut remaining = ASSESSMENT_INPUT_LIMIT;
    let mut documents = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = read_bounded(&path, remaining)?;
        remaining = remaining.saturating_sub(bytes.len() as u64);
        documents.push(bytes);
    }
    Ok(documents)
}

#[cfg(feature = "desktop")]
pub(crate) fn assess_configuration(
    program_kind: ProgramKind,
    bytes: &[u8],
) -> Result<(PrivilegeRequirement, Vec<PrivilegeReason>)> {
    let adapter = AdapterRegistry::default().get(program_kind);
    let Some(reasons) = adapter.assess_privilege_configuration(
        bytes,
        PrivilegeAssessmentContext::for_current_platform(),
    )?
    else {
        return Ok((PrivilegeRequirement::Unknown, Vec::new()));
    };
    let requirement = if reasons.is_empty() {
        PrivilegeRequirement::Standard
    } else {
        PrivilegeRequirement::Elevated
    };
    Ok((requirement, reasons))
}

fn read_bounded(path: &Path, remaining: u64) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path).map_err(CamelliaNexusError::storage)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeConfigUnsafe,
            "Privilege assessment requires a regular configuration file",
        ));
    }
    if metadata.len() > remaining {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeConfigUnsafe,
            "Configuration exceeds the privilege assessment limit",
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(remaining + 1).read_to_end(&mut bytes))
        .map_err(CamelliaNexusError::storage)?;
    if bytes.len() as u64 > remaining {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeConfigUnsafe,
            "Configuration exceeds the privilege assessment limit",
        ));
    }
    Ok(bytes)
}

#[cfg(windows)]
fn windows_manifest_requests_elevation(executable: &Path) -> Result<bool> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            Foundation::FreeLibrary,
            System::LibraryLoader::{
                FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LOAD_LIBRARY_AS_IMAGE_RESOURCE,
                LoadLibraryExW, LoadResource, LockResource, SizeofResource,
            },
            UI::WindowsAndMessaging::RT_MANIFEST,
        },
        core::PCWSTR,
    };

    let path: Vec<u16> = executable
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let module = unsafe {
        LoadLibraryExW(
            PCWSTR(path.as_ptr()),
            None,
            LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE,
        )
    }
    .map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeConfigUnsafe,
            "Could not inspect the executable privilege manifest",
        )
        .with_details(error.to_string())
    })?;
    let result = (|| {
        for identifier in [1usize, 2usize] {
            let resource = unsafe {
                FindResourceW(Some(module), PCWSTR(identifier as *const u16), RT_MANIFEST)
            };
            if resource.is_invalid() {
                continue;
            }
            let size = unsafe { SizeofResource(Some(module), resource) } as usize;
            if size == 0 || size > 1024 * 1024 {
                continue;
            }
            let loaded = unsafe { LoadResource(Some(module), resource) }.map_err(|error| {
                CamelliaNexusError::new(
                    ErrorCode::PrivilegeConfigUnsafe,
                    "Could not load the executable privilege manifest",
                )
                .with_details(error.to_string())
            })?;
            let pointer = unsafe { LockResource(loaded) };
            if pointer.is_null() {
                continue;
            }
            let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size) };
            if manifest_bytes_request_elevation(bytes) {
                return Ok(true);
            }
        }
        Ok(false)
    })();
    unsafe {
        let _ = FreeLibrary(module);
    }
    result
}

#[cfg(windows)]
fn manifest_bytes_request_elevation(bytes: &[u8]) -> bool {
    let ascii = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    if ascii.contains("requireadministrator") || ascii.contains("highestavailable") {
        return true;
    }
    let utf16: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let wide = String::from_utf16_lossy(&utf16).to_ascii_lowercase();
    wide.contains("requireadministrator") || wide.contains("highestavailable")
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use camellia_nexus_core::{
        LaunchPlan, PrivilegeConfigInput, PrivilegePolicy, PrivilegeReason, PrivilegeRequirement,
        ProgramId, ProgramKind,
    };

    use super::assess_launch_plan;
    #[cfg(windows)]
    use super::manifest_bytes_request_elevation;

    fn plan(kind: ProgramKind, config: &std::path::Path) -> LaunchPlan {
        LaunchPlan {
            program_id: ProgramId::parse("privilege-test").expect("id"),
            workspace: config.parent().expect("parent").to_path_buf(),
            managed_executable: true,
            // Windows inspects the executable's embedded manifest before it reads the
            // configuration. Use the running test PE instead of making the configuration
            // file double as an invalid executable fixture.
            executable: std::env::current_exe().expect("test executable"),
            args: Vec::new(),
            cwd: config.parent().expect("parent").to_path_buf(),
            environment: BTreeMap::new(),
            stdout_log: PathBuf::from("stdout.log"),
            stderr_log: PathBuf::from("stderr.log"),
            program_kind: kind,
            privilege_policy: PrivilegePolicy::Automatic,
            privilege_inputs: vec![PrivilegeConfigInput::File {
                path: config.to_path_buf(),
            }],
            interactive: true,
        }
    }

    #[test]
    fn detects_sing_box_tun() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = directory.path().join("config.json");
        std::fs::write(&config, r#"{"inbounds":[{"type":"tun"}]}"#).expect("write");
        let assessment = assess_launch_plan(&plan(ProgramKind::SingBox, &config)).expect("assess");
        assert_eq!(assessment.detected, PrivilegeRequirement::Elevated);
        assert_eq!(assessment.effective, PrivilegeRequirement::Elevated);
        assert!(assessment.reasons.contains(&PrivilegeReason::TunInterface));
    }

    #[test]
    fn detects_tun_in_jsonc_configuration() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = directory.path().join("config.json");
        std::fs::write(
            &config,
            b"{/* user comment */\n\"inbounds\":[{\"type\":\"tun\",}],}",
        )
        .expect("write");
        let assessment = assess_launch_plan(&plan(ProgramKind::SingBox, &config)).expect("assess");
        assert_eq!(assessment.detected, PrivilegeRequirement::Elevated);
        assert!(assessment.reasons.contains(&PrivilegeReason::TunInterface));
    }

    #[test]
    fn detects_tun_in_an_explicit_configuration_directory() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = directory.path().join("base.json");
        std::fs::write(&config, r#"{"inbounds":[{"type":"tun"}]}"#).expect("write");
        let mut launch = plan(ProgramKind::SingBox, &config);
        launch.privilege_inputs = vec![PrivilegeConfigInput::Directory {
            path: directory.path().to_path_buf(),
        }];
        let assessment = assess_launch_plan(&launch).expect("assess");
        assert_eq!(assessment.detected, PrivilegeRequirement::Elevated);
    }

    #[test]
    fn does_not_treat_remote_outbound_ports_as_privileged_listeners() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = directory.path().join("config.json");
        std::fs::write(
            &config,
            r#"{"inbounds":[{"type":"socks","listen_port":1080}],"outbounds":[{"server_port":443}]}"#,
        )
        .expect("write");
        let assessment = assess_launch_plan(&plan(ProgramKind::SingBox, &config)).expect("assess");
        assert_eq!(assessment.detected, PrivilegeRequirement::Standard);
    }

    #[test]
    fn detects_mihomo_tun_and_transparent_proxy_ports() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = directory.path().join("config.yaml");
        std::fs::write(
            &config,
            "tun:\n  enable: true\ntproxy-port: 7894\nmixed-port: 7890\n",
        )
        .expect("write");
        let assessment = assess_launch_plan(&plan(ProgramKind::Mihomo, &config)).expect("assess");
        assert_eq!(assessment.detected, PrivilegeRequirement::Elevated);
        assert!(assessment.reasons.contains(&PrivilegeReason::TunInterface));
        assert!(
            assessment
                .reasons
                .contains(&PrivilegeReason::TransparentProxy)
        );
    }

    #[test]
    fn forced_standard_policy_never_silently_elevates() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = directory.path().join("config.json");
        std::fs::write(&config, r#"{"inbounds":[{"type":"tun"}]}"#).expect("write");
        let mut launch = plan(ProgramKind::SingBox, &config);
        launch.privilege_policy = PrivilegePolicy::Standard;
        let assessment = assess_launch_plan(&launch).expect("assess");
        assert_eq!(assessment.detected, PrivilegeRequirement::Elevated);
        assert_eq!(assessment.effective, PrivilegeRequirement::Standard);
    }

    #[test]
    fn forced_policy_is_not_blocked_by_an_unassessable_external_config() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = directory.path().join("config.json");
        std::fs::write(&config, b"not json").expect("write");
        for policy in [PrivilegePolicy::Standard, PrivilegePolicy::Elevated] {
            let mut launch = plan(ProgramKind::SingBox, &config);
            launch.privilege_policy = policy;
            let assessment = assess_launch_plan(&launch).expect("explicit policy");
            assert_eq!(assessment.detected, PrivilegeRequirement::Unknown);
            assert_eq!(
                assessment.effective,
                if policy == PrivilegePolicy::Standard {
                    PrivilegeRequirement::Standard
                } else {
                    PrivilegeRequirement::Elevated
                }
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn recognizes_ascii_and_utf16_elevation_manifests() {
        assert!(manifest_bytes_request_elevation(
            br#"<requestedExecutionLevel level="requireAdministrator"/>"#,
        ));
        let utf16 = "<requestedExecutionLevel level=\"highestAvailable\"/>"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert!(manifest_bytes_request_elevation(&utf16));
        assert!(!manifest_bytes_request_elevation(
            br#"<requestedExecutionLevel level="asInvoker"/>"#,
        ));
    }
}
