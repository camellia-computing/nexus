use std::{
    io::Write,
    path::{Path, PathBuf},
};

use camellia_nexus_core::{CamelliaNexusError, Result};

#[cfg(unix)]
use tokio::{
    fs::{self, OpenOptions},
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
};

const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
const LOG_FILES: usize = 3;

pub fn prepare_session(paths: [&Path; 2], clear_on_start: bool) -> Result<()> {
    for path in paths {
        prepare_log(path, clear_on_start).map_err(CamelliaNexusError::storage)?;
    }
    Ok(())
}

fn prepare_log(path: &Path, clear_on_start: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if clear_on_start {
        remove_file_if_exists(path)?;
        for index in 1..=LOG_FILES {
            remove_file_if_exists(&numbered(path, index))?;
        }
    } else if std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.len() >= LOG_MAX_BYTES.saturating_sub(256))
    {
        rotate_blocking(path)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    if file.metadata()?.len() > 0 {
        writeln!(file)?;
    }
    writeln!(file, "--- Camellia Nexus session started ---")?;
    file.flush()
}

#[cfg(unix)]
pub async fn capture<R>(mut reader: R, path: PathBuf) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = vec![0u8; 16 * 1024];
    let mut first_error = None;
    let mut output = match open_log(&path).await {
        Ok(output) => Some(output),
        Err(error) => {
            remember_error(&mut first_error, &path, "open", error);
            None
        }
    };
    loop {
        let count = match reader.read(&mut buffer).await {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => {
                remember_error(&mut first_error, &path, "read process output for", error);
                break;
            }
        };
        if output.is_none() {
            output = match open_log(&path).await {
                Ok(output) => Some(output),
                Err(error) => {
                    remember_error(&mut first_error, &path, "reopen", error);
                    None
                }
            };
        }
        if output
            .as_ref()
            .is_some_and(|(_, size)| size.saturating_add(count as u64) > LOG_MAX_BYTES)
        {
            if let Some((mut file, _)) = output.take()
                && let Err(error) = file.flush().await
            {
                remember_error(&mut first_error, &path, "flush", error);
            }
            match rotate(&path).await {
                Ok(()) => match open_log(&path).await {
                    Ok(next) => output = Some(next),
                    Err(error) => remember_error(&mut first_error, &path, "open rotated", error),
                },
                Err(error) => remember_error(&mut first_error, &path, "rotate", error),
            }
        }
        let Some((file, size)) = output.as_mut() else {
            continue;
        };
        match file.write_all(&buffer[..count]).await {
            Ok(()) => *size = size.saturating_add(count as u64),
            Err(error) => {
                remember_error(&mut first_error, &path, "write", error);
                output = None;
            }
        }
    }
    if let Some((mut file, _)) = output
        && let Err(error) = file.flush().await
    {
        remember_error(&mut first_error, &path, "flush", error);
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(unix)]
async fn open_log(path: &Path) -> std::io::Result<(tokio::fs::File, u64)> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    let size = file.metadata().await?.len();
    Ok((file, size))
}

#[cfg(unix)]
async fn rotate(path: &Path) -> std::io::Result<()> {
    for index in (1..LOG_FILES).rev() {
        let from = numbered(path, index);
        let to = numbered(path, index + 1);
        if fs::try_exists(&from).await? {
            remove_file_if_exists_async(&to).await?;
            fs::rename(from, to).await?;
        }
    }
    if fs::try_exists(path).await? {
        let first = numbered(path, 1);
        remove_file_if_exists_async(&first).await?;
        fs::rename(path, first).await?;
    }
    Ok(())
}

fn numbered(path: &Path, index: usize) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{index}"));
    PathBuf::from(value)
}

fn rotate_blocking(path: &Path) -> std::io::Result<()> {
    for index in (1..LOG_FILES).rev() {
        let from = numbered(path, index);
        let to = numbered(path, index + 1);
        if from.exists() {
            remove_file_if_exists(&to)?;
            std::fs::rename(from, to)?;
        }
    }
    if path.exists() {
        let first = numbered(path, 1);
        remove_file_if_exists(&first)?;
        std::fs::rename(path, first)?;
    }
    Ok(())
}

#[cfg(windows)]
pub fn capture_blocking(mut reader: std::fs::File, path: PathBuf) -> std::io::Result<()> {
    use std::io::{Read, Write};

    let mut buffer = vec![0u8; 16 * 1024];
    let mut first_error = None;
    let mut output = match open_log_blocking(&path) {
        Ok(output) => Some(output),
        Err(error) => {
            remember_error(&mut first_error, &path, "open", error);
            None
        }
    };
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) => {
                remember_error(&mut first_error, &path, "read process output for", error);
                break;
            }
        };
        if output.is_none() {
            output = match open_log_blocking(&path) {
                Ok(output) => Some(output),
                Err(error) => {
                    remember_error(&mut first_error, &path, "reopen", error);
                    None
                }
            };
        }
        if output
            .as_ref()
            .is_some_and(|(_, size)| size.saturating_add(count as u64) > LOG_MAX_BYTES)
        {
            if let Some((mut file, _)) = output.take()
                && let Err(error) = file.flush()
            {
                remember_error(&mut first_error, &path, "flush", error);
            }
            match rotate_blocking(&path) {
                Ok(()) => match open_log_blocking(&path) {
                    Ok(next) => output = Some(next),
                    Err(error) => remember_error(&mut first_error, &path, "open rotated", error),
                },
                Err(error) => remember_error(&mut first_error, &path, "rotate", error),
            }
        }
        let Some((file, size)) = output.as_mut() else {
            continue;
        };
        match file.write_all(&buffer[..count]) {
            Ok(()) => *size = size.saturating_add(count as u64),
            Err(error) => {
                remember_error(&mut first_error, &path, "write", error);
                output = None;
            }
        }
    }
    if let Some((mut file, _)) = output
        && let Err(error) = file.flush()
    {
        remember_error(&mut first_error, &path, "flush", error);
    }
    first_error.map_or(Ok(()), Err)
}

fn remove_file_if_exists(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
async fn remove_file_if_exists_async(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remember_error(
    first_error: &mut Option<std::io::Error>,
    path: &Path,
    operation: &str,
    error: std::io::Error,
) {
    if first_error.is_none() {
        *first_error = Some(std::io::Error::new(
            error.kind(),
            format!("failed to {operation} log {}: {error}", path.display()),
        ));
    }
}

#[cfg(windows)]
fn open_log_blocking(path: &Path) -> std::io::Result<(std::fs::File, u64)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let size = file.metadata()?.len();
    Ok((file, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_policy_removes_rotated_history() {
        let directory = tempfile::tempdir().expect("tempdir");
        let stdout = directory.path().join("stdout.log");
        let stderr = directory.path().join("stderr.log");
        std::fs::write(&stdout, "old output").expect("stdout");
        std::fs::write(numbered(&stdout, 1), "older output").expect("rotated stdout");

        prepare_session([&stdout, &stderr], true).expect("prepare logs");

        assert!(!numbered(&stdout, 1).exists());
        assert!(
            !std::fs::read_to_string(stdout)
                .expect("new stdout")
                .contains("old output")
        );
    }

    #[test]
    fn preserve_policy_keeps_output_and_marks_new_session() {
        let directory = tempfile::tempdir().expect("tempdir");
        let stdout = directory.path().join("stdout.log");
        let stderr = directory.path().join("stderr.log");
        std::fs::write(&stdout, "old output").expect("stdout");

        prepare_session([&stdout, &stderr], false).expect("prepare logs");

        let content = std::fs::read_to_string(stdout).expect("preserved stdout");
        assert!(content.contains("old output"));
        assert!(content.contains("Camellia Nexus session"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn capture_reports_log_destination_failures_after_draining_output() {
        let directory = tempfile::tempdir().expect("tempdir");
        let error = capture(tokio::io::empty(), directory.path().to_path_buf())
            .await
            .expect_err("directory is not a writable log file");
        assert!(error.to_string().contains("failed to open log"));
    }
}
