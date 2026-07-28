use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
};

const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;
const RETAINED_ARCHIVES: usize = 5;

#[derive(Clone)]
pub struct RotatingLogWriter {
    state: Option<Arc<Mutex<LogState>>>,
}

struct LogState {
    directory: PathBuf,
    name: String,
    file: Option<File>,
    bytes: u64,
}

impl RotatingLogWriter {
    pub fn new(directory: PathBuf, name: &str) -> Self {
        let state = LogState::open(directory, name).ok();
        Self {
            state: state.map(|state| Arc::new(Mutex::new(state))),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RotatingLogWriter {
    type Writer = LogHandle;

    fn make_writer(&'a self) -> Self::Writer {
        LogHandle {
            state: self.state.clone(),
        }
    }
}

pub struct LogHandle {
    state: Option<Arc<Mutex<LogState>>>,
}

impl Write for LogHandle {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let Some(state) = &self.state else {
            return io::stderr().write(buffer);
        };
        let mut state = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.bytes > 0 && state.bytes.saturating_add(buffer.len() as u64) > MAX_LOG_BYTES {
            state.rotate()?;
        }
        let written = state
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("application log file is unavailable"))?
            .write(buffer)?;
        state.bytes = state.bytes.saturating_add(written as u64);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        let Some(state) = &self.state else {
            return io::stderr().flush();
        };
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("application log file is unavailable"))?
            .flush()
    }
}

impl LogState {
    fn open(directory: PathBuf, name: &str) -> io::Result<Self> {
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(name);
        let bytes = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let mut state = Self {
            directory,
            name: name.to_owned(),
            file: Some(file),
            bytes,
        };
        if state.bytes >= MAX_LOG_BYTES {
            state.rotate()?;
        }
        Ok(state)
    }

    fn rotate(&mut self) -> io::Result<()> {
        if let Some(mut file) = self.file.take() {
            file.flush()?;
        }
        for index in (1..=RETAINED_ARCHIVES).rev() {
            let source = self.archive(index);
            if index == RETAINED_ARCHIVES {
                match std::fs::remove_file(&source) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            } else if source.exists() {
                std::fs::rename(source, self.archive(index + 1))?;
            }
        }
        let current = self.directory.join(&self.name);
        if current.exists() {
            std::fs::rename(&current, self.archive(1))?;
        }
        self.file = Some(OpenOptions::new().create(true).append(true).open(current)?);
        self.bytes = 0;
        Ok(())
    }

    fn archive(&self, index: usize) -> PathBuf {
        self.directory.join(format!("{}.{}", self.name, index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::fmt::MakeWriter;

    #[test]
    fn log_writer_rotates_and_bounds_retained_files() {
        let directory = tempfile::tempdir().expect("temporary log directory");
        let writer = RotatingLogWriter::new(directory.path().to_path_buf(), "app.log");
        for value in 0..=RETAINED_ARCHIVES + 1 {
            let mut handle = writer.make_writer();
            handle
                .write_all(&vec![value as u8; MAX_LOG_BYTES as usize])
                .expect("write full log");
            handle.write_all(b"x").expect("trigger rotation");
        }
        assert!(directory.path().join("app.log").is_file());
        for index in 1..=RETAINED_ARCHIVES {
            assert!(directory.path().join(format!("app.log.{index}")).is_file());
        }
        assert!(
            !directory
                .path()
                .join(format!("app.log.{}", RETAINED_ARCHIVES + 1))
                .exists()
        );
    }
}
