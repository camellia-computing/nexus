pub(crate) mod logging;

use std::{fs, path::Path};

use camellia_nexus_core::{
    CamelliaNexusError, ErrorCode, LaunchPlan, PrivilegeConfigInput, Result,
};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{NativeProcessDriver, NativeToolRunner};
#[cfg(windows)]
pub use windows::{NativeProcessDriver, NativeToolRunner};

pub(crate) fn validate_launch_paths(plan: &LaunchPlan) -> Result<()> {
    if !plan.workspace.is_dir() {
        return Err(invalid_launch_path("Program workspace does not exist"));
    }
    validate_internal_path(&plan.workspace, &plan.stdout_log, false)?;
    validate_internal_path(&plan.workspace, &plan.stderr_log, false)?;
    for input in &plan.privilege_inputs {
        let path = match input {
            PrivilegeConfigInput::File { path } | PrivilegeConfigInput::Directory { path } => path,
        };
        if path.starts_with(&plan.workspace) {
            validate_internal_path(&plan.workspace, path, true)?;
        }
    }
    if plan.managed_executable {
        validate_internal_path(&plan.workspace, &plan.cwd, true)?;
        validate_internal_path(&plan.workspace, &plan.executable, true)?;
        if !plan.executable.starts_with(plan.workspace.join("bin")) {
            return Err(invalid_launch_path(
                "Managed executable is outside the bin directory",
            ));
        }
    } else if !plan.executable.is_file() {
        return Err(invalid_launch_path("External executable does not exist"));
    }
    if !plan.cwd.is_dir() {
        return Err(invalid_launch_path("Working directory does not exist"));
    }
    if !plan.executable.is_file() {
        return Err(invalid_launch_path("Executable does not exist"));
    }
    Ok(())
}

fn validate_internal_path(workspace: &Path, path: &Path, require_leaf: bool) -> Result<()> {
    let relative = path
        .strip_prefix(workspace)
        .map_err(|_| invalid_launch_path("Program resource path is outside its workspace"))?;
    let mut cursor = workspace.to_path_buf();
    for component in relative.components() {
        cursor.push(component.as_os_str());
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if is_link_or_reparse(&metadata) => {
                return Err(invalid_launch_path(
                    "Program resource path contains a link or reparse point",
                ));
            }
            Ok(_) => {}
            Err(error) if !require_leaf && error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(
                    invalid_launch_path("Program resource path is not accessible")
                        .with_details(error.to_string()),
                );
            }
        }
    }
    Ok(())
}

fn invalid_launch_path(message: &str) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::InvalidPath, message)
}

#[cfg(not(windows))]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}
