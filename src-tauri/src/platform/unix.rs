use std::{
    os::fd::{FromRawFd, OwnedFd, RawFd},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use camellia_nexus_core::{
    CamelliaNexusError, CommandOutput, CommandPlan, ErrorCode, LaunchPlan, ManagedProcess,
    PrivilegeRequirement, ProcessDriver, ProcessExit, Result, ToolRunner,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    sync::{Semaphore, mpsc},
    task::JoinHandle,
};

use super::{logging, validate_launch_paths};

#[derive(Clone)]
pub struct NativeProcessDriver {
    clear_logs_on_start: Arc<AtomicBool>,
}

impl NativeProcessDriver {
    pub fn new(clear_logs_on_start: Arc<AtomicBool>) -> Self {
        Self {
            clear_logs_on_start,
        }
    }
}

impl Default for NativeProcessDriver {
    fn default() -> Self {
        Self::new(Arc::new(AtomicBool::new(false)))
    }
}

#[derive(Clone)]
pub struct NativeToolRunner {
    permits: Arc<Semaphore>,
}

impl Default for NativeToolRunner {
    fn default() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(2)),
        }
    }
}

struct UnixManagedProcess {
    child: Child,
    pid: u32,
    pgid: i32,
    stdout_task: Option<JoinHandle<std::io::Result<()>>>,
    stderr_task: Option<JoinHandle<std::io::Result<()>>>,
    _watchdog: ProcessWatchdog,
    exit: Option<ProcessExit>,
}

struct ProcessWatchdog {
    _liveness: OwnedFd,
}

#[async_trait]
impl ProcessDriver for NativeProcessDriver {
    async fn spawn(&self, plan: LaunchPlan) -> Result<Box<dyn ManagedProcess>> {
        validate_launch_paths(&plan)?;
        if crate::privileges::assess_launch_plan(&plan)?.effective == PrivilegeRequirement::Elevated
        {
            return crate::privilege_broker::spawn(
                plan,
                self.clear_logs_on_start.load(Ordering::Acquire),
            )
            .await;
        }
        logging::prepare_session(
            [&plan.stdout_log, &plan.stderr_log],
            self.clear_logs_on_start.load(Ordering::Acquire),
        )?;
        let mut command = Command::new(&plan.executable);
        command
            .args(&plan.args)
            .current_dir(&plan.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_clean_environment(&mut command, &plan.environment);
        configure_process_group(&mut command);
        let mut child = command.spawn().map_err(|error| {
            CamelliaNexusError::new(ErrorCode::SpawnFailed, "Failed to start program")
                .with_details(error.to_string())
        })?;
        let pid = child.id().ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::SpawnFailed, "Started process has no pid")
        })?;
        let watchdog = spawn_process_watchdog(pid as i32).map_err(|error| {
            let _ = signal_group(pid as i32, libc::SIGKILL);
            CamelliaNexusError::new(ErrorCode::SpawnFailed, "Failed to start process watchdog")
                .with_details(error.to_string())
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::SpawnFailed, "Failed to capture stdout")
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::SpawnFailed, "Failed to capture stderr")
        })?;
        let stdout_task = tokio::spawn(logging::capture(stdout, plan.stdout_log));
        let stderr_task = tokio::spawn(logging::capture(stderr, plan.stderr_log));
        Ok(Box::new(UnixManagedProcess {
            child,
            pid,
            pgid: pid as i32,
            stdout_task: Some(stdout_task),
            stderr_task: Some(stderr_task),
            _watchdog: watchdog,
            exit: None,
        }))
    }
}

#[async_trait]
impl ManagedProcess for UnixManagedProcess {
    fn pid(&self) -> u32 {
        self.pid
    }

    async fn wait(&mut self) -> Result<ProcessExit> {
        if let Some(exit) = self.exit {
            return Ok(exit);
        }
        let status = self.child.wait().await.map_err(|error| {
            CamelliaNexusError::new(ErrorCode::Internal, "Failed to wait for program")
                .with_details(error.to_string())
        })?;
        let _ = signal_group(self.pgid, libc::SIGKILL);
        self.finish_logs().await;
        let exit = ProcessExit {
            code: status.code(),
            success: status.success(),
        };
        self.exit = Some(exit);
        Ok(exit)
    }

    async fn stop(&mut self) -> Result<ProcessExit> {
        if let Some(exit) = self.exit {
            return Ok(exit);
        }
        signal_group(self.pgid, libc::SIGTERM)?;
        match tokio::time::timeout(Duration::from_secs(10), self.child.wait()).await {
            Ok(status) => {
                let status = status.map_err(CamelliaNexusError::storage)?;
                let _ = signal_group(self.pgid, libc::SIGKILL);
                self.finish_logs().await;
                let exit = ProcessExit {
                    code: status.code(),
                    success: status.success(),
                };
                self.exit = Some(exit);
                Ok(exit)
            }
            Err(_) => {
                signal_group(self.pgid, libc::SIGKILL)?;
                let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
                    .await
                    .map_err(|_| {
                        CamelliaNexusError::new(
                            ErrorCode::StopFailed,
                            "Program did not exit after SIGKILL",
                        )
                    })?
                    .map_err(CamelliaNexusError::storage)?;
                self.finish_logs().await;
                let exit = ProcessExit {
                    code: status.code(),
                    success: status.success(),
                };
                self.exit = Some(exit);
                Ok(exit)
            }
        }
    }
}

impl UnixManagedProcess {
    async fn finish_logs(&mut self) {
        finish_log_task(self.stdout_task.take()).await;
        finish_log_task(self.stderr_task.take()).await;
    }
}

async fn finish_log_task(task: Option<JoinHandle<std::io::Result<()>>>) {
    let Some(mut task) = task else { return };
    match tokio::time::timeout(Duration::from_secs(1), &mut task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => tracing::error!(%error, "program log capture failed"),
        Ok(Err(error)) => tracing::error!(%error, "program log capture task failed"),
        Err(_) => {
            tracing::warn!("program log capture did not finish before timeout");
            task.abort();
        }
    }
}

impl Drop for UnixManagedProcess {
    fn drop(&mut self) {
        if self.exit.is_none() {
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
        }
        if let Some(task) = self.stdout_task.take() {
            task.abort();
        }
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }
    }
}

#[async_trait]
impl ToolRunner for NativeToolRunner {
    async fn run(&self, plan: CommandPlan) -> Result<CommandOutput> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(CamelliaNexusError::internal)?;
        let timeout = plan.timeout;
        let mut command = Command::new(&plan.executable);
        command
            .args(&plan.args)
            .current_dir(&plan.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        apply_clean_environment(&mut command, &plan.environment);
        configure_process_group(&mut command);
        let mut child = command.spawn().map_err(|error| {
            CamelliaNexusError::new(ErrorCode::SpawnFailed, "Failed to start tool command")
                .with_details(error.to_string())
        })?;
        let pgid = child.id().ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::SpawnFailed, "Started tool has no pid")
        })? as i32;
        let stdout = child.stdout.take().ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::Internal, "Tool stdout was not captured")
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            CamelliaNexusError::new(ErrorCode::Internal, "Tool stderr was not captured")
        })?;
        let (violation_tx, mut violation_rx) = mpsc::channel(2);
        let total_output = Arc::new(AtomicUsize::new(0));
        let stdout_task = tokio::spawn(collect_limited(
            stdout,
            plan.max_output_bytes,
            total_output.clone(),
            violation_tx.clone(),
        ));
        let stderr_task = tokio::spawn(collect_limited(
            stderr,
            plan.max_output_bytes,
            total_output.clone(),
            violation_tx,
        ));

        enum Completion {
            Exited(std::process::ExitStatus),
            Limit,
            Timeout,
        }
        let completion = tokio::select! {
            status = child.wait() => Completion::Exited(status.map_err(CamelliaNexusError::storage)?),
            Some(()) = violation_rx.recv() => Completion::Limit,
            _ = tokio::time::sleep(timeout) => Completion::Timeout,
        };
        let status = match completion {
            Completion::Exited(status) => status,
            Completion::Limit => {
                let _ = signal_group(pgid, libc::SIGKILL);
                let _ = child.wait().await;
                let _ = finish_collector(stdout_task).await;
                let _ = finish_collector(stderr_task).await;
                return Err(CamelliaNexusError::new(
                    ErrorCode::OutputLimitExceeded,
                    "Tool output exceeded its configured limit",
                ));
            }
            Completion::Timeout => {
                let _ = signal_group(pgid, libc::SIGKILL);
                let _ = child.wait().await;
                let _ = finish_collector(stdout_task).await;
                let _ = finish_collector(stderr_task).await;
                return Err(CamelliaNexusError::new(
                    ErrorCode::Timeout,
                    format!("Tool command timed out after {} ms", timeout.as_millis()),
                ));
            }
        };
        let _ = signal_group(pgid, libc::SIGKILL);
        let stdout = finish_collector(stdout_task).await?;
        let stderr = finish_collector(stderr_task).await?;
        if total_output.load(Ordering::Acquire) > plan.max_output_bytes {
            return Err(CamelliaNexusError::new(
                ErrorCode::OutputLimitExceeded,
                "Tool output exceeded its configured limit",
            ));
        }
        Ok(CommandOutput {
            code: status.code(),
            success: status.success(),
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }
}

fn apply_clean_environment(
    command: &mut Command,
    overrides: &std::collections::BTreeMap<String, String>,
) {
    const INHERITED_KEYS: &[&str] = &[
        "HOME",
        "LANG",
        "LC_ALL",
        "PATH",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
    ];
    command.env_clear();
    for key in INHERITED_KEYS {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    command.envs(overrides);
}

fn configure_process_group(command: &mut Command) {
    #[cfg(target_os = "linux")]
    let parent = unsafe { libc::getpid() };
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    libc::_exit(1);
                }
            }
            Ok(())
        });
    }
}

fn spawn_process_watchdog(pgid: i32) -> std::io::Result<ProcessWatchdog> {
    let mut pipe = [0 as RawFd; 2];
    unsafe {
        if libc::pipe(pipe.as_mut_ptr()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        for fd in pipe {
            if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) == -1 {
                let error = std::io::Error::last_os_error();
                libc::close(pipe[0]);
                libc::close(pipe[1]);
                return Err(error);
            }
        }
        let max_fd = libc::sysconf(libc::_SC_OPEN_MAX);
        let max_fd = if max_fd > 0 {
            max_fd.min(RawFd::MAX as libc::c_long) as RawFd
        } else {
            1024
        };
        let first = libc::fork();
        if first == -1 {
            let error = std::io::Error::last_os_error();
            libc::close(pipe[0]);
            libc::close(pipe[1]);
            return Err(error);
        }
        if first == 0 {
            let second = libc::fork();
            if second != 0 {
                libc::_exit(if second == -1 { 1 } else { 0 });
            }
            let Some(liveness) = isolate_watchdog_liveness(pipe[0], pipe[1], max_fd) else {
                libc::_exit(1);
            };
            let mut byte = 0_u8;
            loop {
                let result = libc::read(liveness, (&mut byte as *mut u8).cast(), 1);
                if result == -1 {
                    continue;
                }
                break;
            }
            libc::kill(-pgid, libc::SIGKILL);
            libc::close(liveness);
            libc::_exit(0);
        }
        libc::close(pipe[0]);
        let mut status = 0;
        if libc::waitpid(first, &mut status, 0) == -1
            || !libc::WIFEXITED(status)
            || libc::WEXITSTATUS(status) != 0
        {
            let error = std::io::Error::other("process watchdog could not detach");
            libc::close(pipe[1]);
            return Err(error);
        }
        Ok(ProcessWatchdog {
            _liveness: OwnedFd::from_raw_fd(pipe[1]),
        })
    }
}

fn isolate_watchdog_liveness(read_fd: RawFd, write_fd: RawFd, max_fd: RawFd) -> Option<RawFd> {
    const LIVENESS_FD: RawFd = libc::STDIN_FILENO;
    unsafe {
        libc::close(write_fd);
        if read_fd != LIVENESS_FD {
            if libc::dup2(read_fd, LIVENESS_FD) == -1 {
                return None;
            }
            libc::close(read_fd);
        }

        #[cfg(target_os = "linux")]
        if libc::syscall(
            libc::SYS_close_range,
            1 as libc::c_uint,
            libc::c_uint::MAX,
            0 as libc::c_uint,
        ) == 0
        {
            return Some(LIVENESS_FD);
        }

        for fd in 1..max_fd {
            libc::close(fd);
        }
    }
    Some(LIVENESS_FD)
}

fn signal_group(pgid: i32, signal: i32) -> Result<()> {
    let result = unsafe { libc::kill(-pgid, signal) };
    if result == 0 {
        Ok(())
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            Err(
                CamelliaNexusError::new(ErrorCode::StopFailed, "Failed to signal process group")
                    .with_details(error.to_string()),
            )
        }
    }
}

async fn collect_limited<R>(
    mut reader: R,
    limit: usize,
    total: Arc<AtomicUsize>,
    violation: mpsc::Sender<()>,
) -> Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = vec![0u8; 16 * 1024];
    let mut reported = false;
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(CamelliaNexusError::storage)?;
        if count == 0 {
            break;
        }
        let previous = total
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_add(count))
            })
            .unwrap_or_else(|value| value);
        let remaining = limit.saturating_sub(previous);
        output.extend_from_slice(&buffer[..count.min(remaining)]);
        if count > remaining && !reported {
            reported = true;
            let _ = violation.send(()).await;
        }
    }
    Ok(output)
}

async fn finish_collector(mut task: JoinHandle<Result<Vec<u8>>>) -> Result<Vec<u8>> {
    match tokio::time::timeout(Duration::from_secs(1), &mut task).await {
        Ok(result) => result.map_err(CamelliaNexusError::internal)?,
        Err(_) => {
            task.abort();
            Err(CamelliaNexusError::new(
                ErrorCode::Internal,
                "Tool output pipe did not close",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
        path::PathBuf,
    };

    use camellia_nexus_core::{CommandPlan, LaunchPlan, ProcessDriver, ProgramId, ToolRunner};

    use super::{NativeProcessDriver, NativeToolRunner, spawn_process_watchdog};

    #[test]
    fn watchdog_does_not_retain_unrelated_file_descriptors() {
        let mut pipe = [-1; 2];
        unsafe {
            assert_eq!(libc::pipe(pipe.as_mut_ptr()), 0);
        }
        let read = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
        let watchdog = spawn_process_watchdog(i32::MAX).expect("watchdog");

        drop(write);
        let mut descriptor = libc::pollfd {
            fd: read.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut descriptor, 1, 1_000) };

        drop(watchdog);
        assert_eq!(ready, 1, "watchdog retained an unrelated pipe writer");
        assert_ne!(descriptor.revents & libc::POLLHUP, 0);
    }

    #[tokio::test]
    async fn tool_runner_captures_stdout_and_stderr() {
        let runner = NativeToolRunner::default();
        for _ in 0..32 {
            let plan = CommandPlan::tool(
                PathBuf::from("/bin/sh"),
                vec!["-c".into(), "printf hello; printf error >&2".into()],
                PathBuf::from("/tmp"),
            );
            let output = runner.run(plan).await.expect("tool should run");
            assert!(output.success);
            assert_eq!(output.stdout, "hello");
            assert_eq!(output.stderr, "error");
        }
    }

    #[tokio::test]
    async fn tool_output_limit_applies_to_both_streams_combined() {
        let runner = NativeToolRunner::default();
        let mut plan = CommandPlan::tool(
            PathBuf::from("/bin/sh"),
            vec!["-c".into(), "printf 1234; printf 5678 >&2".into()],
            PathBuf::from("/tmp"),
        );
        plan.max_output_bytes = 6;

        let error = runner.run(plan).await.expect_err("combined output limit");
        assert_eq!(
            error.code,
            camellia_nexus_core::ErrorCode::OutputLimitExceeded
        );
    }

    #[tokio::test]
    async fn process_driver_stops_process_group() {
        let directory = tempfile::tempdir().expect("tempdir");
        let driver = NativeProcessDriver::default();
        let plan = LaunchPlan {
            program_id: ProgramId::parse("fixture").expect("id"),
            workspace: directory.path().to_path_buf(),
            managed_executable: false,
            executable: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "sleep 30 & wait".into()],
            cwd: directory.path().to_path_buf(),
            environment: BTreeMap::new(),
            stdout_log: directory.path().join("stdout.log"),
            stderr_log: directory.path().join("stderr.log"),
            program_kind: camellia_nexus_core::ProgramKind::Generic,
            privilege_policy: Default::default(),
            privilege_inputs: Vec::new(),
            interactive: true,
        };
        let mut process = driver.spawn(plan).await.expect("spawn");
        assert!(process.pid() > 0);
        tokio::time::timeout(std::time::Duration::from_secs(2), process.stop())
            .await
            .expect("stop timeout")
            .expect("stop");
    }

    #[tokio::test]
    async fn root_exit_terminates_descendants_that_hold_log_pipes() {
        let directory = tempfile::tempdir().expect("tempdir");
        let driver = NativeProcessDriver::default();
        let plan = LaunchPlan {
            program_id: ProgramId::parse("descendant-fixture").expect("id"),
            workspace: directory.path().to_path_buf(),
            managed_executable: false,
            executable: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), "sleep 30 &".into()],
            cwd: directory.path().to_path_buf(),
            environment: BTreeMap::new(),
            stdout_log: directory.path().join("stdout.log"),
            stderr_log: directory.path().join("stderr.log"),
            program_kind: camellia_nexus_core::ProgramKind::Generic,
            privilege_policy: Default::default(),
            privilege_inputs: Vec::new(),
            interactive: true,
        };
        let mut process = driver.spawn(plan).await.expect("spawn");
        tokio::time::timeout(std::time::Duration::from_secs(2), process.wait())
            .await
            .expect("wait must not be held by descendant pipes")
            .expect("wait");
    }
}
