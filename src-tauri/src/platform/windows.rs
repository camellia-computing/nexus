//! Windows process implementation based on CreateProcessW + Job Objects.

use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString, c_void},
    fs::File,
    io::Read,
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    path::Path,
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
    sync::{Semaphore, mpsc, oneshot},
    task::JoinHandle,
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0,
        },
        Security::SECURITY_ATTRIBUTES,
        System::{
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject, TerminateJobObject,
            },
            Pipes::CreatePipe,
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
                DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
                INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION, ResumeThread,
                STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, TerminateProcess,
                UpdateProcThreadAttribute, WaitForSingleObject,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

use super::{logging, validate_launch_paths};

const WINDOWS_PROCESS_STRING_LIMIT: usize = 32_767;

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

struct OwnedHandle(HANDLE);

unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    fn into_file(self) -> File {
        let raw = self.0.0;
        std::mem::forget(self);
        unsafe { File::from_raw_handle(raw) }
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

struct ProcThreadAttributeList {
    _storage: Vec<usize>,
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
}

impl ProcThreadAttributeList {
    fn for_inherited_handles(handles: &[HANDLE]) -> Result<Self> {
        let mut bytes = 0usize;
        // The sizing call is documented to fail with insufficient buffer while
        // returning the required allocation size.
        let _ = unsafe { InitializeProcThreadAttributeList(None, 1, None, &mut bytes) };
        if bytes == 0 {
            return Err(CamelliaNexusError::new(
                ErrorCode::SpawnFailed,
                "Could not size the Windows process attribute list",
            ));
        }
        let words = bytes.div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0usize; words];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(storage.as_mut_ptr().cast());
        unsafe { InitializeProcThreadAttributeList(Some(list), 1, None, &mut bytes) }.map_err(
            |error| {
                CamelliaNexusError::new(
                    ErrorCode::SpawnFailed,
                    "Could not initialize the Windows process attribute list",
                )
                .with_details(error.to_string())
            },
        )?;
        if let Err(error) = unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                Some(handles.as_ptr().cast()),
                std::mem::size_of_val(handles),
                None,
                None,
            )
        } {
            unsafe { DeleteProcThreadAttributeList(list) };
            return Err(CamelliaNexusError::new(
                ErrorCode::SpawnFailed,
                "Could not restrict inherited Windows process handles",
            )
            .with_details(error.to_string()));
        }
        Ok(Self {
            _storage: storage,
            list,
        })
    }
}

impl Drop for ProcThreadAttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

struct SpawnedWindowsProcess {
    pid: u32,
    job: OwnedHandle,
    exit_rx: oneshot::Receiver<Result<ProcessExit>>,
    stdout: File,
    stderr: File,
}

struct WindowsManagedProcess {
    pid: u32,
    job: OwnedHandle,
    exit_rx: oneshot::Receiver<Result<ProcessExit>>,
    exit: Option<ProcessExit>,
    stdout_task: Option<JoinHandle<std::io::Result<()>>>,
    stderr_task: Option<JoinHandle<std::io::Result<()>>>,
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
        let spawned = spawn_suspended(&plan.executable, &plan.args, &plan.cwd, &plan.environment)?;
        let stdout_task = tokio::task::spawn_blocking({
            let path = plan.stdout_log;
            move || logging::capture_blocking(spawned.stdout, path)
        });
        let stderr_task = tokio::task::spawn_blocking({
            let path = plan.stderr_log;
            move || logging::capture_blocking(spawned.stderr, path)
        });
        Ok(Box::new(WindowsManagedProcess {
            pid: spawned.pid,
            job: spawned.job,
            exit_rx: spawned.exit_rx,
            exit: None,
            stdout_task: Some(stdout_task),
            stderr_task: Some(stderr_task),
        }))
    }
}

#[async_trait]
impl ManagedProcess for WindowsManagedProcess {
    fn pid(&self) -> u32 {
        self.pid
    }

    async fn wait(&mut self) -> Result<ProcessExit> {
        if let Some(exit) = self.exit {
            return Ok(exit);
        }
        let exit = (&mut self.exit_rx).await.map_err(|_| {
            CamelliaNexusError::new(ErrorCode::Internal, "Windows wait task stopped")
        })??;
        unsafe {
            let _ = TerminateJobObject(self.job.0, 1);
        }
        self.finish_logs().await;
        self.exit = Some(exit);
        Ok(exit)
    }

    async fn stop(&mut self) -> Result<ProcessExit> {
        if self.exit.is_none() {
            unsafe {
                TerminateJobObject(self.job.0, 1).map_err(|error| {
                    CamelliaNexusError::new(
                        ErrorCode::StopFailed,
                        "Failed to terminate Windows Job",
                    )
                    .with_details(error.to_string())
                })?;
            }
        }
        tokio::time::timeout(Duration::from_secs(5), self.wait())
            .await
            .map_err(|_| {
                CamelliaNexusError::new(
                    ErrorCode::StopFailed,
                    "Windows process did not exit after Job termination",
                )
            })?
    }
}

impl WindowsManagedProcess {
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

#[async_trait]
impl ToolRunner for NativeToolRunner {
    async fn run(&self, plan: CommandPlan) -> Result<CommandOutput> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(CamelliaNexusError::internal)?;
        let timeout = plan.timeout;
        let spawned = spawn_suspended(&plan.executable, &plan.args, &plan.cwd, &plan.environment)?;
        let SpawnedWindowsProcess {
            job,
            mut exit_rx,
            stdout,
            stderr,
            ..
        } = spawned;
        let (limit_tx, mut limit_rx) = mpsc::unbounded_channel();
        let total_output = Arc::new(AtomicUsize::new(0));
        let stdout_task = tokio::task::spawn_blocking({
            let tx = limit_tx.clone();
            let limit = plan.max_output_bytes;
            let total = total_output.clone();
            move || collect_limited(stdout, limit, total, tx)
        });
        let stderr_task = tokio::task::spawn_blocking({
            let limit = plan.max_output_bytes;
            let total = total_output.clone();
            move || collect_limited(stderr, limit, total, limit_tx)
        });

        enum Completion {
            Exited(Result<ProcessExit>),
            Limit,
            Timeout,
        }
        let completion = tokio::select! {
            result = &mut exit_rx => Completion::Exited(result.map_err(|_| {
                CamelliaNexusError::new(ErrorCode::Internal, "Windows wait task stopped")
            })?),
            Some(()) = limit_rx.recv() => Completion::Limit,
            _ = tokio::time::sleep(timeout) => Completion::Timeout,
        };
        let exit = match completion {
            Completion::Exited(exit) => exit?,
            Completion::Limit => {
                unsafe {
                    let _ = TerminateJobObject(job.0, 1);
                }
                let _ = exit_rx.await;
                let _ = finish_collector(stdout_task).await;
                let _ = finish_collector(stderr_task).await;
                return Err(CamelliaNexusError::new(
                    ErrorCode::OutputLimitExceeded,
                    "Tool output exceeded its configured limit",
                ));
            }
            Completion::Timeout => {
                unsafe {
                    let _ = TerminateJobObject(job.0, 1);
                }
                let _ = exit_rx.await;
                let _ = finish_collector(stdout_task).await;
                let _ = finish_collector(stderr_task).await;
                return Err(CamelliaNexusError::new(
                    ErrorCode::Timeout,
                    format!("Tool command timed out after {} ms", timeout.as_millis()),
                ));
            }
        };
        unsafe {
            let _ = TerminateJobObject(job.0, 1);
        }
        let stdout = finish_collector(stdout_task).await?;
        let stderr = finish_collector(stderr_task).await?;
        if total_output.load(Ordering::Acquire) > plan.max_output_bytes {
            return Err(CamelliaNexusError::new(
                ErrorCode::OutputLimitExceeded,
                "Tool output exceeded its configured limit",
            ));
        }
        Ok(CommandOutput {
            code: exit.code,
            success: exit.success,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }
}

fn spawn_suspended(
    executable: &Path,
    args: &[String],
    cwd: &Path,
    environment: &BTreeMap<String, String>,
) -> Result<SpawnedWindowsProcess> {
    let job = create_kill_on_close_job()?;
    let (stdin_read, stdin_write) = create_stdin_pipe()?;
    let (stdout_read, stdout_write) = create_inherited_pipe()?;
    let (stderr_read, stderr_write) = create_inherited_pipe()?;
    let application = wide_null(executable.as_os_str());
    let mut command_line = build_command_line(executable.as_os_str(), args)?;
    let current_directory = wide_null(cwd.as_os_str());
    let environment = build_environment(environment)?;
    let inherited_handles = [stdin_read.0, stdout_write.0, stderr_write.0];
    let attributes = ProcThreadAttributeList::for_inherited_handles(&inherited_handles)?;
    let startup = STARTUPINFOEXW {
        StartupInfo: STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOEXW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdOutput: stdout_write.0,
            hStdError: stderr_write.0,
            hStdInput: stdin_read.0,
            ..Default::default()
        },
        lpAttributeList: attributes.list,
    };
    let mut information = PROCESS_INFORMATION::default();
    let flags = CREATE_SUSPENDED
        | CREATE_NO_WINDOW
        | CREATE_UNICODE_ENVIRONMENT
        | EXTENDED_STARTUPINFO_PRESENT;
    unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command_line.as_mut_ptr())),
            None,
            None,
            true,
            flags,
            Some(environment.as_ptr() as *const c_void),
            PCWSTR(current_directory.as_ptr()),
            &startup.StartupInfo,
            &mut information,
        )
        .map_err(|error| {
            CamelliaNexusError::new(ErrorCode::SpawnFailed, "CreateProcessW failed")
                .with_details(error.to_string())
        })?;
    }
    drop(stdin_read);
    drop(stdin_write);
    drop(stdout_write);
    drop(stderr_write);
    let process = OwnedHandle(information.hProcess);
    let thread = OwnedHandle(information.hThread);
    let assigned = unsafe { AssignProcessToJobObject(job.0, process.0) };
    if let Err(error) = assigned {
        unsafe {
            let _ = TerminateProcess(process.0, 1);
        }
        return Err(CamelliaNexusError::new(
            ErrorCode::SpawnFailed,
            "Failed to assign process to Windows Job",
        )
        .with_details(error.to_string()));
    }
    let resume_result = unsafe { ResumeThread(thread.0) };
    if resume_result == u32::MAX {
        unsafe {
            let _ = TerminateJobObject(job.0, 1);
        }
        return Err(CamelliaNexusError::new(
            ErrorCode::SpawnFailed,
            "Failed to resume Windows process",
        ));
    }
    drop(thread);
    let pid = information.dwProcessId;
    let (exit_tx, exit_rx) = oneshot::channel();
    tokio::task::spawn_blocking(move || {
        let result = wait_for_process(&process);
        let _ = exit_tx.send(result);
    });
    Ok(SpawnedWindowsProcess {
        pid,
        job,
        exit_rx,
        stdout: stdout_read.into_file(),
        stderr: stderr_read.into_file(),
    })
}

fn create_kill_on_close_job() -> Result<OwnedHandle> {
    let job =
        OwnedHandle(unsafe { CreateJobObjectW(None, None) }.map_err(CamelliaNexusError::storage)?);
    let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &information as *const _ as *const c_void,
            std::mem::size_of_val(&information) as u32,
        )
        .map_err(CamelliaNexusError::storage)?;
    }
    Ok(job)
}

fn create_inherited_pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    unsafe {
        CreatePipe(&mut read, &mut write, Some(&attributes), 0)
            .map_err(CamelliaNexusError::storage)?;
        if let Err(error) = SetHandleInformation(read, HANDLE_FLAG_INHERIT.0, Default::default()) {
            let _ = CloseHandle(read);
            let _ = CloseHandle(write);
            return Err(CamelliaNexusError::storage(error));
        }
    }
    Ok((OwnedHandle(read), OwnedHandle(write)))
}

fn create_stdin_pipe() -> Result<(OwnedHandle, OwnedHandle)> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: true.into(),
    };
    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    unsafe {
        CreatePipe(&mut read, &mut write, Some(&attributes), 0)
            .map_err(CamelliaNexusError::storage)?;
        if let Err(error) = SetHandleInformation(write, HANDLE_FLAG_INHERIT.0, Default::default()) {
            let _ = CloseHandle(read);
            let _ = CloseHandle(write);
            return Err(CamelliaNexusError::storage(error));
        }
    }
    Ok((OwnedHandle(read), OwnedHandle(write)))
}

fn wait_for_process(process: &OwnedHandle) -> Result<ProcessExit> {
    let wait = unsafe { WaitForSingleObject(process.0, INFINITE) };
    if wait != WAIT_OBJECT_0 {
        return Err(CamelliaNexusError::new(
            ErrorCode::Internal,
            "WaitForSingleObject failed",
        ));
    }
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(process.0, &mut code) }.map_err(CamelliaNexusError::storage)?;
    Ok(ProcessExit {
        code: Some(code as i32),
        success: code == 0,
    })
}

fn collect_limited(
    mut reader: File,
    limit: usize,
    total: Arc<AtomicUsize>,
    violation: mpsc::UnboundedSender<()>,
) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = vec![0u8; 16 * 1024];
    let mut reported = false;
    loop {
        let count = reader
            .read(&mut buffer)
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
            let _ = violation.send(());
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

fn build_command_line(executable: &OsStr, args: &[String]) -> Result<Vec<u16>> {
    let mut line = quote_windows(executable);
    for arg in args {
        line.push(' ');
        line.push_str(&quote_windows(OsStr::new(arg)));
    }
    let encoded: Vec<_> = OsStr::new(&line).encode_wide().chain(Some(0)).collect();
    if encoded.len() > WINDOWS_PROCESS_STRING_LIMIT {
        return Err(CamelliaNexusError::invalid_spec(
            "Windows command line exceeds 32767 UTF-16 units",
        ));
    }
    Ok(encoded)
}

fn quote_windows(value: &OsStr) -> String {
    let value = value.to_string_lossy();
    if !value.is_empty() && !value.chars().any(|c| c.is_whitespace() || c == '"') {
        return value.into_owned();
    }
    let mut quoted = String::from("\"");
    let mut slashes = 0usize;
    for character in value.chars() {
        match character {
            '\\' => slashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(slashes * 2 + 1));
                quoted.push('"');
                slashes = 0;
            }
            other => {
                quoted.push_str(&"\\".repeat(slashes));
                slashes = 0;
                quoted.push(other);
            }
        }
    }
    quoted.push_str(&"\\".repeat(slashes * 2));
    quoted.push('"');
    quoted
}

fn build_environment(overrides: &BTreeMap<String, String>) -> Result<Vec<u16>> {
    const INHERITED_KEYS: &[&str] = &[
        "APPDATA",
        "COMSPEC",
        "LOCALAPPDATA",
        "PATH",
        "PATHEXT",
        "SYSTEMROOT",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "WINDIR",
    ];
    let mut environment: BTreeMap<String, OsString> = INHERITED_KEYS
        .iter()
        .filter_map(|key| std::env::var_os(key).map(|value| ((*key).to_owned(), value)))
        .collect();
    for (key, value) in overrides {
        environment.insert(key.to_uppercase(), OsString::from(value));
    }
    let mut block = Vec::new();
    for (key, value) in environment {
        block.extend(OsStr::new(&key).encode_wide());
        block.push('=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    if block.len() > WINDOWS_PROCESS_STRING_LIMIT {
        return Err(CamelliaNexusError::invalid_spec(
            "Windows environment block exceeds 32767 UTF-16 units",
        ));
    }
    Ok(block)
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_quoting_handles_spaces_quotes_and_slashes() {
        assert_eq!(quote_windows(OsStr::new("plain")), "plain");
        assert_eq!(quote_windows(OsStr::new("two words")), "\"two words\"");
        assert_eq!(quote_windows(OsStr::new("a\\\"b")), "\"a\\\\\\\"b\"");
    }

    #[test]
    fn windows_process_strings_are_bounded() {
        let oversized = "x".repeat(WINDOWS_PROCESS_STRING_LIMIT);
        assert!(build_command_line(OsStr::new("program.exe"), &[oversized]).is_err());

        let environment = BTreeMap::from([("OVERSIZED".to_owned(), "x".repeat(32_768))]);
        assert!(build_environment(&environment).is_err());
    }

    #[tokio::test]
    async fn fast_tools_do_not_treat_closed_output_channels_as_limits() {
        let runner = NativeToolRunner::default();
        let cwd = std::env::current_dir().expect("current directory");
        let command = std::env::var_os("ComSpec")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("cmd.exe"));
        for _ in 0..32 {
            let plan = CommandPlan::tool(
                command.clone(),
                vec!["/C".into(), "exit 0".into()],
                cwd.clone(),
            );
            let output = runner.run(plan).await.expect("tool should run");
            assert!(output.success);
        }
    }

    #[tokio::test]
    async fn concurrent_tools_keep_inherited_output_handles_isolated() {
        let runner = NativeToolRunner::default();
        let cwd = std::env::current_dir().expect("current directory");
        let command = std::env::var_os("ComSpec")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("cmd.exe"));
        let mut tasks = Vec::new();
        for index in 0..16 {
            let runner = runner.clone();
            let command = command.clone();
            let cwd = cwd.clone();
            tasks.push(tokio::spawn(async move {
                let token = format!("camellia-handle-{index}");
                let plan =
                    CommandPlan::tool(command, vec!["/C".into(), format!("echo {token}")], cwd);
                (token, runner.run(plan).await)
            }));
        }
        for task in tasks {
            let (token, output) = task.await.expect("tool task");
            let output = output.expect("tool output");
            assert!(output.success);
            assert_eq!(output.stdout.trim(), token);
            assert!(output.stderr.is_empty());
        }
    }
}
