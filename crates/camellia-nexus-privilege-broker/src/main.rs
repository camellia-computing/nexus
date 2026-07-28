use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{BufRead, BufReader, BufWriter, Write},
    net::{SocketAddr, TcpStream},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use camellia_nexus_core::{
    CamelliaNexusError, ErrorCode, LaunchPlan, PRIVILEGE_BROKER_PROTOCOL_VERSION,
    PrivilegeBrokerEvent, PrivilegeBrokerRequest, ProcessExit, ProgramId,
};
use sha2::{Digest, Sha256};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const COMMAND_LIMIT: usize = 1024 * 1024;
const STOP_GRACE: Duration = Duration::from_secs(10);

fn main() {
    if let Err(error) = run() {
        eprintln!("privilege broker failed: {}", error.message);
        std::process::exit(1);
    }
}

fn run() -> camellia_nexus_core::Result<()> {
    let (address, nonce) = parse_arguments()?;
    require_elevated_identity()?;
    let stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerFailed,
            "The privilege broker could not connect to Camellia Nexus",
        )
        .with_details(error.to_string())
    })?;
    run_broker(stream, nonce)
}

fn run_broker(stream: TcpStream, nonce: String) -> camellia_nexus_core::Result<()> {
    stream
        .set_nodelay(true)
        .map_err(CamelliaNexusError::storage)?;
    let reader_stream = stream.try_clone().map_err(CamelliaNexusError::storage)?;
    let mut writer = BufWriter::new(stream);
    write_event(
        &mut writer,
        &PrivilegeBrokerEvent::Hello {
            protocol_version: PRIVILEGE_BROKER_PROTOCOL_VERSION,
            nonce,
            broker_pid: std::process::id(),
        },
    )?;
    let mut reader = BufReader::new(reader_stream);
    let (command_tx, command_rx) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            match read_request(&mut reader) {
                Ok(request) => {
                    if command_tx.send(Some(request)).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = command_tx.send(None);
                    break;
                }
            }
        }
    });

    let mut children = ManagedChildren::default();
    let mut grants: HashMap<ProgramId, [u8; 32]> = HashMap::new();
    loop {
        let mut exited = Vec::new();
        for (program_id, child) in children.iter_mut() {
            if let Some(status) = child.try_wait().map_err(CamelliaNexusError::storage)? {
                exited.push((program_id.clone(), process_exit(status)));
            }
        }
        for (program_id, exit) in exited {
            children.remove(&program_id);
            write_event(
                &mut writer,
                &PrivilegeBrokerEvent::Exited { program_id, exit },
            )?;
        }
        match command_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Some(PrivilegeBrokerRequest::Launch {
                protocol_version,
                request_id,
                plan,
            })) => {
                let program_id = plan.program_id.clone();
                let result = (|| {
                    if protocol_version != PRIVILEGE_BROKER_PROTOCOL_VERSION {
                        return Err(protocol_error("Privilege broker protocol version mismatch"));
                    }
                    validate_request_id(&request_id)?;
                    validate_plan(&plan)?;
                    if children.contains_key(&program_id) {
                        return Err(protocol_error("The privileged program is already running"));
                    }
                    let fingerprint = plan_fingerprint(&plan)?;
                    if grants
                        .get(&program_id)
                        .is_some_and(|granted| granted != &fingerprint)
                    {
                        return Err(CamelliaNexusError::new(
                            ErrorCode::PrivilegeConfigUnsafe,
                            "The privileged launch definition changed during this administrator session",
                        ));
                    }
                    let child = spawn_managed(&plan)?;
                    let pid = child.id();
                    grants.entry(program_id.clone()).or_insert(fingerprint);
                    children.insert(program_id.clone(), child);
                    Ok(pid)
                })();
                match result {
                    Ok(pid) => write_event(
                        &mut writer,
                        &PrivilegeBrokerEvent::Started {
                            request_id,
                            program_id,
                            pid,
                        },
                    )?,
                    Err(error) => write_event(
                        &mut writer,
                        &PrivilegeBrokerEvent::Failed {
                            request_id,
                            program_id: Some(program_id),
                            error,
                        },
                    )?,
                }
            }
            Ok(Some(PrivilegeBrokerRequest::Stop {
                request_id,
                program_id,
            })) => {
                validate_request_id(&request_id)?;
                let Some(mut child) = children.remove(&program_id) else {
                    write_event(
                        &mut writer,
                        &PrivilegeBrokerEvent::Failed {
                            request_id,
                            program_id: Some(program_id),
                            error: protocol_error("The privileged program is not running"),
                        },
                    )?;
                    continue;
                };
                match stop_managed(&mut child) {
                    Ok(exit) => write_event(
                        &mut writer,
                        &PrivilegeBrokerEvent::Exited { program_id, exit },
                    )?,
                    Err(error) => {
                        children.insert(program_id.clone(), child);
                        write_event(
                            &mut writer,
                            &PrivilegeBrokerEvent::Failed {
                                request_id,
                                program_id: Some(program_id),
                                error,
                            },
                        )?;
                    }
                }
            }
            Ok(Some(PrivilegeBrokerRequest::Shutdown { request_id })) => {
                validate_request_id(&request_id)?;
                while let Some(program_id) = children.keys().next().cloned() {
                    let mut child = children
                        .remove(&program_id)
                        .expect("selected privileged child must remain present");
                    let exit = match stop_managed(&mut child) {
                        Ok(exit) => exit,
                        Err(stop_error) => {
                            if terminate_managed(&mut child).is_err() {
                                children.insert(program_id, child);
                                return Err(stop_error);
                            }
                            match child.wait() {
                                Ok(status) => process_exit(status),
                                Err(_) => {
                                    children.insert(program_id, child);
                                    return Err(stop_error);
                                }
                            }
                        }
                    };
                    write_event(
                        &mut writer,
                        &PrivilegeBrokerEvent::Exited { program_id, exit },
                    )?;
                }
                return Ok(());
            }
            Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                for child in children.values_mut() {
                    let _ = terminate_managed(child);
                }
                return Err(CamelliaNexusError::new(
                    ErrorCode::PrivilegeBrokerConnectionLost,
                    "Camellia Nexus disconnected from the privilege broker",
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

#[derive(Default)]
struct ManagedChildren(HashMap<ProgramId, ManagedChild>);

impl std::ops::Deref for ManagedChildren {
    type Target = HashMap<ProgramId, ManagedChild>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ManagedChildren {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ManagedChildren {
    fn drop(&mut self) {
        for child in self.0.values_mut() {
            let _ = terminate_managed(child);
        }
    }
}

fn validate_request_id(value: &str) -> camellia_nexus_core::Result<()> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| protocol_error("Invalid privilege broker request identity"))?;
    if parsed.get_version() == Some(uuid::Version::Random)
        && parsed.get_variant() == uuid::Variant::RFC4122
        && parsed.hyphenated().to_string() == value
    {
        Ok(())
    } else {
        Err(protocol_error("Invalid privilege broker request identity"))
    }
}

fn plan_fingerprint(plan: &LaunchPlan) -> camellia_nexus_core::Result<[u8; 32]> {
    let mut canonical = plan.clone();
    canonical.interactive = false;
    let bytes = serde_json::to_vec(&canonical).map_err(CamelliaNexusError::storage)?;
    Ok(Sha256::digest(bytes).into())
}

fn parse_arguments() -> camellia_nexus_core::Result<(SocketAddr, String)> {
    let mut arguments = std::env::args();
    let _program = arguments.next();
    if arguments.next().as_deref() != Some("--broker") {
        return Err(protocol_error(
            "The privilege broker accepts only broker mode",
        ));
    }
    let address: SocketAddr = arguments
        .next()
        .ok_or_else(|| protocol_error("Missing broker address"))?
        .parse()
        .map_err(|_| protocol_error("Invalid broker address"))?;
    if !address.ip().is_loopback() {
        return Err(protocol_error("The broker address must be loopback-only"));
    }
    let nonce = arguments
        .next()
        .filter(|value| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| protocol_error("Invalid broker nonce"))?;
    if arguments.next().is_some() {
        return Err(protocol_error("Unexpected privilege broker arguments"));
    }
    Ok((address, nonce))
}

fn validate_plan(plan: &LaunchPlan) -> camellia_nexus_core::Result<()> {
    if !plan.workspace.is_absolute()
        || !plan.executable.is_absolute()
        || !plan.cwd.is_absolute()
        || !plan.stdout_log.is_absolute()
        || !plan.stderr_log.is_absolute()
        || !plan.workspace.is_dir()
        || !plan.cwd.is_dir()
        || !plan.executable.is_file()
        || !plan.stdout_log.starts_with(&plan.workspace)
        || !plan.stderr_log.starts_with(&plan.workspace)
    {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeConfigUnsafe,
            "The privileged launch manifest contains an invalid path",
        ));
    }
    if plan.args.len() > 256
        || plan.environment.len() > 128
        || plan
            .args
            .iter()
            .any(|argument| argument.contains('\0') || argument.len() > 32 * 1024)
        || plan.environment.iter().any(|(key, value)| {
            key.is_empty()
                || key.contains(['\0', '='])
                || value.contains('\0')
                || key.len() > 512
                || value.len() > 32 * 1024
        })
    {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeConfigUnsafe,
            "The privileged launch manifest exceeds its safety limits",
        ));
    }
    reject_link_components(&plan.workspace, &plan.stdout_log)?;
    reject_link_components(&plan.workspace, &plan.stderr_log)?;
    if plan.managed_executable {
        reject_link_components(&plan.workspace, &plan.executable)?;
        reject_link_components(&plan.workspace, &plan.cwd)?;
        if !plan.executable.starts_with(plan.workspace.join("bin")) {
            return Err(CamelliaNexusError::new(
                ErrorCode::PrivilegeConfigUnsafe,
                "A managed privileged executable must remain inside its bin directory",
            ));
        }
    }
    Ok(())
}

fn reject_link_components(root: &Path, target: &Path) -> camellia_nexus_core::Result<()> {
    let relative = target.strip_prefix(root).map_err(|_| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeConfigUnsafe,
            "A privileged resource is outside its workspace",
        )
    })?;
    let mut cursor = root.to_path_buf();
    for component in relative.components() {
        cursor.push(component.as_os_str());
        match std::fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(CamelliaNexusError::new(
                    ErrorCode::PrivilegeConfigUnsafe,
                    "A privileged resource path contains a symbolic link",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(CamelliaNexusError::storage(error)),
        }
    }
    Ok(())
}

struct ManagedChild {
    child: Child,
    #[cfg(windows)]
    job: WindowsJob,
}

impl ManagedChild {
    fn id(&self) -> u32 {
        self.child.id()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait()
    }
}

fn spawn_managed(plan: &LaunchPlan) -> camellia_nexus_core::Result<ManagedChild> {
    let stdout = open_log(&plan.stdout_log)?;
    let stderr = open_log(&plan.stderr_log)?;
    let mut command = Command::new(&plan.executable);
    command
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .env_clear()
        .envs(&plan.environment);
    configure_managed_process(&mut command);
    let child = command.spawn().map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerFailed,
            "The privilege broker could not start the program",
        )
        .with_details(error.to_string())
    })?;
    #[cfg(windows)]
    let job = {
        let job = WindowsJob::attach(&child)?;
        resume_managed(&child)?;
        job
    };
    Ok(ManagedChild {
        child,
        #[cfg(windows)]
        job,
    })
}

fn open_log(path: &Path) -> camellia_nexus_core::Result<std::fs::File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(CamelliaNexusError::storage)
}

#[cfg(unix)]
fn configure_managed_process(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(windows)]
fn configure_managed_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    use windows::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_SUSPENDED};

    // Keep the child suspended until it is inside the kill-on-close Job Object. This closes the
    // process-tree escape window between CreateProcess and AssignProcessToJobObject. Do not request
    // BREAKAWAY_FROM_JOB: launchers may place the broker in a Job that rejects breakaway, while
    // supported Windows versions allow the suspended child to join this nested kill-on-close Job.
    command.creation_flags((CREATE_SUSPENDED | CREATE_NEW_PROCESS_GROUP).0);
}

#[cfg(windows)]
fn resume_managed(child: &Child) -> camellia_nexus_core::Result<()> {
    use windows::Win32::{
        Foundation::CloseHandle,
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }.map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerFailed,
            "The privilege broker could not inspect the suspended process",
        )
        .with_details(error.to_string())
    })?;
    let result = (|| {
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        unsafe { Thread32First(snapshot, &mut entry) }.map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::PrivilegeBrokerFailed,
                "The privilege broker could not enumerate the suspended process",
            )
            .with_details(error.to_string())
        })?;
        loop {
            if entry.th32OwnerProcessID == child.id() {
                let thread =
                    unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) }
                        .map_err(|error| {
                            CamelliaNexusError::new(
                                ErrorCode::PrivilegeBrokerFailed,
                                "The privilege broker could not open the suspended process thread",
                            )
                            .with_details(error.to_string())
                        })?;
                let resumed = unsafe { ResumeThread(thread) };
                unsafe {
                    let _ = CloseHandle(thread);
                }
                if resumed == u32::MAX {
                    return Err(CamelliaNexusError::new(
                        ErrorCode::PrivilegeBrokerFailed,
                        "The privilege broker could not resume the managed process",
                    ));
                }
                return Ok(());
            }
            if unsafe { Thread32Next(snapshot, &mut entry) }.is_err() {
                return Err(CamelliaNexusError::new(
                    ErrorCode::PrivilegeBrokerFailed,
                    "The suspended process did not expose its primary thread",
                ));
            }
        }
    })();
    unsafe {
        let _ = CloseHandle(snapshot);
    }
    result
}

#[cfg(unix)]
fn terminate_managed(child: &mut ManagedChild) -> std::io::Result<()> {
    let result = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn terminate_managed(child: &mut ManagedChild) -> std::io::Result<()> {
    child.job.terminate()
}

fn stop_managed(child: &mut ManagedChild) -> camellia_nexus_core::Result<ProcessExit> {
    #[cfg(unix)]
    unsafe {
        let _ = libc::kill(-(child.id() as i32), libc::SIGTERM);
    }
    #[cfg(windows)]
    child.job.terminate().map_err(CamelliaNexusError::storage)?;

    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().map_err(CamelliaNexusError::storage)? {
            return Ok(process_exit(status));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    terminate_managed(child).map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::StopFailed,
            "The privileged program did not stop before forced termination",
        )
        .with_details(error.to_string())
    })?;
    let status = child.wait().map_err(CamelliaNexusError::storage)?;
    Ok(process_exit(status))
}

#[cfg(windows)]
struct WindowsJob(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl WindowsJob {
    fn attach(child: &Child) -> camellia_nexus_core::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::{
            Foundation::HANDLE,
            System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
        };
        let job = unsafe { CreateJobObjectW(None, None) }.map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::PrivilegeBrokerFailed,
                "The privilege broker could not create a Windows Job",
            )
            .with_details(error.to_string())
        })?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Err(error) = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(job);
            }
            return Err(CamelliaNexusError::new(
                ErrorCode::PrivilegeBrokerFailed,
                "The privilege broker could not configure its Windows Job",
            )
            .with_details(error.to_string()));
        }
        let process = HANDLE(child.as_raw_handle());
        if let Err(error) = unsafe { AssignProcessToJobObject(job, process) } {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(job);
            }
            return Err(CamelliaNexusError::new(
                ErrorCode::PrivilegeBrokerFailed,
                "The privileged process could not be assigned to its Windows Job",
            )
            .with_details(error.to_string()));
        }
        Ok(Self(job))
    }

    fn terminate(&self) -> std::io::Result<()> {
        unsafe { windows::Win32::System::JobObjects::TerminateJobObject(self.0, 1) }
            .map_err(std::io::Error::other)
    }
}

#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn process_exit(status: ExitStatus) -> ProcessExit {
    ProcessExit {
        code: status.code(),
        success: status.success(),
    }
}

fn read_request(reader: &mut impl BufRead) -> camellia_nexus_core::Result<PrivilegeBrokerRequest> {
    let mut line = String::new();
    let read = std::io::Read::take(reader, (COMMAND_LIMIT + 1) as u64)
        .read_line(&mut line)
        .map_err(CamelliaNexusError::storage)?;
    if read == 0 || read > COMMAND_LIMIT || !line.ends_with('\n') {
        return Err(protocol_error("Invalid or oversized broker request"));
    }
    serde_json::from_str(&line).map_err(|error| protocol_error(error.to_string()))
}

fn write_event(
    writer: &mut BufWriter<TcpStream>,
    event: &PrivilegeBrokerEvent,
) -> camellia_nexus_core::Result<()> {
    serde_json::to_writer(&mut *writer, event).map_err(CamelliaNexusError::storage)?;
    writer
        .write_all(b"\n")
        .map_err(CamelliaNexusError::storage)?;
    writer.flush().map_err(CamelliaNexusError::storage)
}

fn protocol_error(message: impl Into<String>) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::PrivilegeBrokerFailed, message)
}

#[cfg(unix)]
fn require_elevated_identity() -> camellia_nexus_core::Result<()> {
    if unsafe { libc::geteuid() } == 0 {
        Ok(())
    } else {
        Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeRequired,
            "The privilege broker was not started as root",
        ))
    }
}

#[cfg(windows)]
fn require_elevated_identity() -> camellia_nexus_core::Result<()> {
    use windows::Win32::{
        Foundation::CloseHandle,
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut token = Default::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeRequired,
            "Could not inspect broker elevation",
        )
        .with_details(error.to_string())
    })?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0;
    let result = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    unsafe {
        let _ = CloseHandle(token);
    }
    result.map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeRequired,
            "Could not inspect broker elevation",
        )
        .with_details(error.to_string())
    })?;
    if elevation.TokenIsElevated != 0 {
        Ok(())
    } else {
        Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeRequired,
            "The privilege broker was not elevated",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        io::{BufRead, BufReader, BufWriter, Write},
        net::{TcpListener, TcpStream},
        path::PathBuf,
        time::Duration,
    };

    use camellia_nexus_core::{
        LaunchPlan, PRIVILEGE_BROKER_PROTOCOL_VERSION, PrivilegeBrokerEvent,
        PrivilegeBrokerRequest, PrivilegeConfigInput, PrivilegePolicy, ProgramId, ProgramKind,
    };

    use super::{plan_fingerprint, run_broker, validate_plan, validate_request_id};

    fn launch_plan(workspace: &std::path::Path) -> LaunchPlan {
        let executable = std::env::current_exe().expect("test executable");
        LaunchPlan {
            program_id: ProgramId::parse("broker-test").expect("id"),
            workspace: workspace.to_path_buf(),
            managed_executable: false,
            executable,
            args: Vec::new(),
            cwd: workspace.to_path_buf(),
            environment: BTreeMap::new(),
            stdout_log: workspace.join("stdout.log"),
            stderr_log: workspace.join("stderr.log"),
            program_kind: ProgramKind::Generic,
            privilege_policy: PrivilegePolicy::Elevated,
            privilege_inputs: Vec::new(),
            interactive: true,
        }
    }

    #[test]
    fn accepts_one_bounded_external_launch_manifest() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(validate_plan(&launch_plan(directory.path())).is_ok());
    }

    #[test]
    fn rejects_log_destinations_outside_the_program_workspace() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut plan = launch_plan(directory.path());
        plan.stdout_log = PathBuf::from(if cfg!(windows) {
            r"C:\outside.log"
        } else {
            "/outside.log"
        });
        assert!(validate_plan(&plan).is_err());
    }

    #[test]
    fn rejects_oversized_argument_vectors() {
        let directory = tempfile::tempdir().expect("tempdir");
        let mut plan = launch_plan(directory.path());
        plan.args = vec!["argument".into(); 257];
        assert!(validate_plan(&plan).is_err());
    }

    #[test]
    fn broker_request_ids_are_canonical_random_uuids() {
        let request_id = uuid::Uuid::new_v4().to_string();
        assert!(validate_request_id(&request_id).is_ok());
        assert!(validate_request_id(&request_id.to_uppercase()).is_err());
        assert!(validate_request_id("00000000-0000-0000-0000-000000000000").is_err());
    }

    #[test]
    fn session_grant_fingerprint_ignores_interaction_but_not_launch_content() {
        let directory = tempfile::tempdir().expect("tempdir");
        let plan = launch_plan(directory.path());
        let mut non_interactive = plan.clone();
        non_interactive.interactive = false;
        assert_eq!(
            plan_fingerprint(&plan).expect("interactive fingerprint"),
            plan_fingerprint(&non_interactive).expect("non-interactive fingerprint")
        );

        let mut changed = plan;
        changed.args.push("--changed".to_owned());
        assert_ne!(
            plan_fingerprint(&changed).expect("changed fingerprint"),
            plan_fingerprint(&non_interactive).expect("original fingerprint")
        );
    }

    #[test]
    fn broker_session_runs_multiple_processes_and_restarts_after_config_updates() {
        let directory = tempfile::tempdir().expect("tempdir");
        let configuration = directory.path().join("config.json");
        std::fs::write(&configuration, b"{\"revision\":1}").expect("initial configuration");

        let mut first = launch_plan(directory.path());
        first.program_id = ProgramId::parse("session-first").expect("first id");
        first.args = vec![
            "--ignored".to_owned(),
            "--exact".to_owned(),
            "tests::privileged_child_fixture".to_owned(),
        ];
        first.environment = BTreeMap::from([(
            "CAMELLIA_NEXUS_PRIVILEGED_CHILD_FIXTURE".to_owned(),
            "1".to_owned(),
        )]);
        first.privilege_inputs = vec![PrivilegeConfigInput::File {
            path: configuration.clone(),
        }];

        let mut second = first.clone();
        second.program_id = ProgramId::parse("session-second").expect("second id");
        second.stdout_log = directory.path().join("second-stdout.log");
        second.stderr_log = directory.path().join("second-stderr.log");

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("broker listener");
        let address = listener.local_addr().expect("broker address");
        let client = TcpStream::connect(address).expect("broker client");
        client
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        let (server, _) = listener.accept().expect("broker server");
        let nonce = "0123456789abcdef0123456789abcdef".to_owned();
        let broker = std::thread::spawn(move || run_broker(server, nonce).expect("broker session"));
        let mut reader = BufReader::new(client.try_clone().expect("reader clone"));
        let mut writer = BufWriter::new(client);

        assert!(matches!(
            read_test_event(&mut reader),
            PrivilegeBrokerEvent::Hello {
                protocol_version: PRIVILEGE_BROKER_PROTOCOL_VERSION,
                ..
            }
        ));

        let first_launch = uuid::Uuid::new_v4().to_string();
        send_test_request(
            &mut writer,
            &PrivilegeBrokerRequest::Launch {
                protocol_version: PRIVILEGE_BROKER_PROTOCOL_VERSION,
                request_id: first_launch.clone(),
                plan: Box::new(first.clone()),
            },
        );
        let first_pid = expect_started(
            read_test_event(&mut reader),
            &first_launch,
            &first.program_id,
        );

        let second_launch = uuid::Uuid::new_v4().to_string();
        send_test_request(
            &mut writer,
            &PrivilegeBrokerRequest::Launch {
                protocol_version: PRIVILEGE_BROKER_PROTOCOL_VERSION,
                request_id: second_launch.clone(),
                plan: Box::new(second.clone()),
            },
        );
        let second_pid = expect_started(
            read_test_event(&mut reader),
            &second_launch,
            &second.program_id,
        );
        assert_ne!(first_pid, second_pid);

        stop_test_program(&mut reader, &mut writer, &first.program_id);
        std::fs::write(&configuration, b"{\"revision\":2}").expect("updated configuration");
        first.interactive = false;
        let restart_request = uuid::Uuid::new_v4().to_string();
        send_test_request(
            &mut writer,
            &PrivilegeBrokerRequest::Launch {
                protocol_version: PRIVILEGE_BROKER_PROTOCOL_VERSION,
                request_id: restart_request.clone(),
                plan: Box::new(first.clone()),
            },
        );
        let restarted_pid = expect_started(
            read_test_event(&mut reader),
            &restart_request,
            &first.program_id,
        );
        assert_ne!(first_pid, restarted_pid);

        stop_test_program(&mut reader, &mut writer, &first.program_id);
        stop_test_program(&mut reader, &mut writer, &second.program_id);
        send_test_request(
            &mut writer,
            &PrivilegeBrokerRequest::Shutdown {
                request_id: uuid::Uuid::new_v4().to_string(),
            },
        );
        drop(writer);
        drop(reader);
        broker.join().expect("join broker");
    }

    #[test]
    #[ignore = "launched as a real child by the broker session regression"]
    fn privileged_child_fixture() {
        assert_eq!(
            std::env::var("CAMELLIA_NEXUS_PRIVILEGED_CHILD_FIXTURE").as_deref(),
            Ok("1")
        );
        std::thread::sleep(Duration::from_secs(60));
    }

    fn send_test_request(writer: &mut BufWriter<TcpStream>, request: &PrivilegeBrokerRequest) {
        serde_json::to_writer(&mut *writer, request).expect("serialize request");
        writer.write_all(b"\n").expect("terminate request");
        writer.flush().expect("flush request");
    }

    fn read_test_event(reader: &mut BufReader<TcpStream>) -> PrivilegeBrokerEvent {
        let mut line = String::new();
        reader.read_line(&mut line).expect("read broker event");
        assert!(!line.is_empty(), "broker closed before the expected event");
        serde_json::from_str(&line).expect("parse broker event")
    }

    fn expect_started(
        event: PrivilegeBrokerEvent,
        expected_request: &str,
        expected_program: &ProgramId,
    ) -> u32 {
        match event {
            PrivilegeBrokerEvent::Started {
                request_id,
                program_id,
                pid,
            } => {
                assert_eq!(request_id, expected_request);
                assert_eq!(&program_id, expected_program);
                pid
            }
            other => panic!("expected broker start, received {other:?}"),
        }
    }

    fn stop_test_program(
        reader: &mut BufReader<TcpStream>,
        writer: &mut BufWriter<TcpStream>,
        program_id: &ProgramId,
    ) {
        send_test_request(
            writer,
            &PrivilegeBrokerRequest::Stop {
                request_id: uuid::Uuid::new_v4().to_string(),
                program_id: program_id.clone(),
            },
        );
        match read_test_event(reader) {
            PrivilegeBrokerEvent::Exited {
                program_id: exited, ..
            } => assert_eq!(&exited, program_id),
            other => panic!("expected broker exit, received {other:?}"),
        }
    }
}
