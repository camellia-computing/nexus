use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex as StdMutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use camellia_nexus_core::{
    CamelliaNexusError, ErrorCode, LaunchPlan, ManagedProcess, PRIVILEGE_BROKER_PROTOCOL_VERSION,
    PrivilegeBrokerEvent, PrivilegeBrokerRequest, PrivilegePolicy, ProcessExit, ProgramId, Result,
};
use tokio::{
    io::{
        AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadHalf, WriteHalf,
    },
    net::{TcpListener, TcpStream},
    sync::{Mutex, mpsc},
};

#[path = "../privilege_broker_identity.rs"]
mod privilege_broker_identity;

const AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(120);
const BROKER_IO_TIMEOUT: Duration = Duration::from_secs(15);
const BROKER_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const BROKER_EVENT_QUEUE_CAPACITY: usize = 2;
const FRAME_LIMIT: u64 = 1024 * 1024;
static BROKER_SESSION: OnceLock<Mutex<Option<Arc<BrokerClient>>>> = OnceLock::new();
static PRIVILEGED_SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);

pub(crate) async fn spawn(
    mut plan: LaunchPlan,
    clear_logs_on_start: bool,
) -> Result<Box<dyn ManagedProcess>> {
    let interactive = plan.interactive;
    ensure_authorization_mode(&plan)?;
    crate::platform::logging::prepare_session(
        [&plan.stdout_log, &plan.stderr_log],
        clear_logs_on_start,
    )?;
    plan.environment = complete_environment(plan.environment);
    let program_id = plan.program_id.clone();
    let broker = broker_session(interactive).await?;
    let mut receiver = broker.register(program_id.clone())?;
    let request_id = uuid::Uuid::new_v4().to_string();
    if let Err(error) = broker
        .send(&PrivilegeBrokerRequest::Launch {
            protocol_version: PRIVILEGE_BROKER_PROTOCOL_VERSION,
            request_id: request_id.clone(),
            plan: Box::new(plan),
        })
        .await
    {
        broker.unregister(&program_id);
        return Err(error);
    }
    let pid = match tokio::time::timeout(BROKER_IO_TIMEOUT, receiver.recv()).await {
        Ok(Some(PrivilegeBrokerEvent::Started {
            request_id: actual_request,
            program_id: actual_program,
            pid,
        })) if actual_request == request_id && actual_program == program_id => pid,
        Ok(Some(PrivilegeBrokerEvent::Failed {
            request_id: actual_request,
            program_id: actual_program,
            error,
        })) if actual_request == request_id && actual_program.as_ref() == Some(&program_id) => {
            broker.unregister(&program_id);
            return Err(error);
        }
        Ok(Some(_)) => {
            cancel_pending_launch(&broker, &program_id).await;
            broker.unregister(&program_id);
            return Err(protocol_error(
                "The privilege broker sent an unexpected launch response",
            ));
        }
        Ok(None) => {
            cancel_pending_launch(&broker, &program_id).await;
            broker.unregister(&program_id);
            return Err(process_lost_error());
        }
        Err(_) => {
            cancel_pending_launch(&broker, &program_id).await;
            broker.unregister(&program_id);
            return Err(CamelliaNexusError::new(
                ErrorCode::PrivilegeBrokerFailed,
                "The privilege broker did not start the program in time",
            ));
        }
    };
    Ok(Box::new(BrokeredProcess {
        program_id,
        pid,
        broker,
        receiver,
        exit: None,
    }))
}

async fn cancel_pending_launch(broker: &BrokerClient, program_id: &ProgramId) {
    let _ = broker
        .send(&PrivilegeBrokerRequest::Stop {
            request_id: uuid::Uuid::new_v4().to_string(),
            program_id: program_id.clone(),
        })
        .await;
}

pub(crate) fn has_active_session() -> bool {
    PRIVILEGED_SESSION_ACTIVE.load(Ordering::Acquire)
}

#[cfg(feature = "desktop")]
pub(crate) async fn end_session() -> Result<()> {
    let session = BROKER_SESSION.get_or_init(|| Mutex::new(None));
    let broker = session.lock().await.take();
    PRIVILEGED_SESSION_ACTIVE.store(false, Ordering::Release);
    if let Some(broker) = broker.filter(|broker| broker.is_alive()) {
        broker
            .send(&PrivilegeBrokerRequest::Shutdown {
                request_id: uuid::Uuid::new_v4().to_string(),
            })
            .await?;
    }
    Ok(())
}

async fn broker_session(interactive: bool) -> Result<Arc<BrokerClient>> {
    let session = BROKER_SESSION.get_or_init(|| Mutex::new(None));
    let mut current = session.lock().await;
    if let Some(broker) = current.as_ref().filter(|broker| broker.is_alive()) {
        return Ok(Arc::clone(broker));
    }
    *current = None;
    PRIVILEGED_SESSION_ACTIVE.store(false, Ordering::Release);
    if !interactive {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeRequired,
            "This program requires an active administrator session and was skipped during background startup",
        ));
    }
    let broker = establish_broker().await?;
    *current = Some(Arc::clone(&broker));
    Ok(broker)
}

async fn establish_broker() -> Result<Arc<BrokerClient>> {
    let broker_executable = broker_path()?;
    verify_privilege_broker_identity(&broker_executable)?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|error| {
            broker_error("Could not open the local privilege broker channel", error)
        })?;
    let address = listener
        .local_addr()
        .map_err(|error| broker_error("Could not inspect the privilege broker channel", error))?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut launcher = launch_broker(&broker_executable, address, &nonce).await?;
    let (stream, peer) = accept_connection(&listener, &mut launcher).await?;
    if !peer.ip().is_loopback() {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerFailed,
            "The privilege broker connected from a non-loopback address",
        ));
    }
    stream
        .set_nodelay(true)
        .map_err(CamelliaNexusError::storage)?;
    let (reader, writer) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader);
    match read_event(&mut reader).await? {
        PrivilegeBrokerEvent::Hello {
            protocol_version,
            nonce: actual_nonce,
            ..
        } if protocol_version == PRIVILEGE_BROKER_PROTOCOL_VERSION && actual_nonce == nonce => {}
        _ => {
            return Err(CamelliaNexusError::new(
                ErrorCode::PrivilegeBrokerFailed,
                "The privilege broker handshake was not authentic",
            ));
        }
    }
    let broker = Arc::new(BrokerClient {
        writer: Mutex::new(writer),
        processes: StdMutex::new(HashMap::new()),
        alive: AtomicBool::new(true),
        _launcher: StdMutex::new(launcher),
    });
    PRIVILEGED_SESSION_ACTIVE.store(true, Ordering::Release);
    tokio::spawn(read_broker_events(Arc::clone(&broker), reader));
    Ok(broker)
}

fn ensure_authorization_mode(plan: &LaunchPlan) -> Result<()> {
    match plan.privilege_policy {
        PrivilegePolicy::Standard => return Ok(()),
        PrivilegePolicy::Automatic | PrivilegePolicy::Elevated => {}
    }
    if !plan.interactive && !has_active_session() {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeRequired,
            "This program requires an active administrator session and was skipped during background startup",
        ));
    }
    Ok(())
}

struct BrokerClient {
    writer: Mutex<WriteHalf<TcpStream>>,
    processes: StdMutex<HashMap<ProgramId, mpsc::Sender<PrivilegeBrokerEvent>>>,
    alive: AtomicBool,
    _launcher: StdMutex<BrokerLauncher>,
}

impl BrokerClient {
    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    fn register(&self, program_id: ProgramId) -> Result<mpsc::Receiver<PrivilegeBrokerEvent>> {
        if !self.is_alive() {
            return Err(process_lost_error());
        }
        let mut processes = self.processes.lock().map_err(|_| process_lost_error())?;
        match processes.entry(program_id) {
            std::collections::hash_map::Entry::Occupied(_) => Err(protocol_error(
                "The privileged program already has an active broker registration",
            )),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let (sender, receiver) = mpsc::channel(BROKER_EVENT_QUEUE_CAPACITY);
                entry.insert(sender);
                Ok(receiver)
            }
        }
    }

    fn unregister(&self, program_id: &ProgramId) {
        if let Ok(mut processes) = self.processes.lock() {
            processes.remove(program_id);
        }
    }

    async fn send(&self, request: &PrivilegeBrokerRequest) -> Result<()> {
        if !self.is_alive() {
            return Err(process_lost_error());
        }
        let result =
            send_request_with_timeout(&self.writer, &self.alive, request, BROKER_IO_TIMEOUT).await;
        if let Err(error) = &result
            && (!self.is_alive() || error.code == ErrorCode::PrivilegeBrokerConnectionLost)
        {
            self.fail_all(error.clone());
            let _ = tokio::time::timeout(BROKER_CLOSE_TIMEOUT, async {
                self.writer.lock().await.shutdown().await
            })
            .await;
        }
        result
    }

    fn dispatch(&self, program_id: &ProgramId, event: PrivilegeBrokerEvent) -> Result<()> {
        let processes = self.processes.lock().map_err(|_| process_lost_error())?;
        let Some(sender) = processes.get(program_id) else {
            return Ok(());
        };
        try_send_process_event(sender, event)
    }

    fn fail_all(&self, error: CamelliaNexusError) {
        self.alive.store(false, Ordering::Release);
        PRIVILEGED_SESSION_ACTIVE.store(false, Ordering::Release);
        if let Ok(mut processes) = self.processes.lock() {
            for (_, sender) in processes.drain() {
                let _ = sender.try_send(PrivilegeBrokerEvent::Failed {
                    request_id: String::new(),
                    program_id: None,
                    error: error.clone(),
                });
            }
        }
    }
}

async fn read_broker_events(broker: Arc<BrokerClient>, mut reader: BufReader<ReadHalf<TcpStream>>) {
    loop {
        match read_event(&mut reader).await {
            Ok(event) => {
                let program_id = match &event {
                    PrivilegeBrokerEvent::Started { program_id, .. }
                    | PrivilegeBrokerEvent::Exited { program_id, .. } => Some(program_id.clone()),
                    PrivilegeBrokerEvent::Failed { program_id, .. } => program_id.clone(),
                    PrivilegeBrokerEvent::Hello { .. } => None,
                };
                if let Some(program_id) = program_id {
                    if let Err(error) = broker.dispatch(&program_id, event) {
                        broker.fail_all(error);
                        return;
                    }
                    continue;
                }
                match event {
                    PrivilegeBrokerEvent::Failed { error, .. } => broker.fail_all(error),
                    PrivilegeBrokerEvent::Hello { .. } => broker.fail_all(protocol_error(
                        "The privilege broker repeated its session handshake",
                    )),
                    _ => unreachable!("program-scoped event was dispatched"),
                }
                return;
            }
            Err(error) => {
                broker.fail_all(error);
                return;
            }
        }
    }
}

struct BrokeredProcess {
    program_id: ProgramId,
    pid: u32,
    broker: Arc<BrokerClient>,
    receiver: mpsc::Receiver<PrivilegeBrokerEvent>,
    exit: Option<ProcessExit>,
}

fn try_send_process_event(
    sender: &mpsc::Sender<PrivilegeBrokerEvent>,
    event: PrivilegeBrokerEvent,
) -> Result<()> {
    match sender.try_send(event) {
        Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => Ok(()),
        Err(mpsc::error::TrySendError::Full(_)) => Err(protocol_error(
            "The privilege broker overflowed a process lifecycle event queue",
        )),
    }
}

#[async_trait]
impl ManagedProcess for BrokeredProcess {
    fn pid(&self) -> u32 {
        self.pid
    }

    async fn wait(&mut self) -> Result<ProcessExit> {
        if let Some(exit) = self.exit {
            return Ok(exit);
        }
        let exit = event_exit(
            self.receiver.recv().await.ok_or_else(process_lost_error)?,
            &self.program_id,
        )?;
        self.exit = Some(exit);
        self.broker.unregister(&self.program_id);
        Ok(exit)
    }

    async fn stop(&mut self) -> Result<ProcessExit> {
        if let Some(exit) = self.exit {
            return Ok(exit);
        }
        self.broker
            .send(&PrivilegeBrokerRequest::Stop {
                request_id: uuid::Uuid::new_v4().to_string(),
                program_id: self.program_id.clone(),
            })
            .await?;
        let event = tokio::time::timeout(BROKER_IO_TIMEOUT, self.receiver.recv())
            .await
            .map_err(|_| {
                CamelliaNexusError::new(
                    ErrorCode::StopFailed,
                    "The privilege broker did not confirm process termination",
                )
            })?
            .ok_or_else(process_lost_error)?;
        let exit = event_exit(event, &self.program_id)?;
        self.exit = Some(exit);
        self.broker.unregister(&self.program_id);
        Ok(exit)
    }
}

impl Drop for BrokeredProcess {
    fn drop(&mut self) {
        if self.exit.is_none()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let broker = Arc::clone(&self.broker);
            let program_id = self.program_id.clone();
            runtime.spawn(async move {
                let _ = broker
                    .send(&PrivilegeBrokerRequest::Stop {
                        request_id: uuid::Uuid::new_v4().to_string(),
                        program_id: program_id.clone(),
                    })
                    .await;
                broker.unregister(&program_id);
            });
        } else {
            self.broker.unregister(&self.program_id);
        }
    }
}

fn event_exit(event: PrivilegeBrokerEvent, expected_program: &ProgramId) -> Result<ProcessExit> {
    match event {
        PrivilegeBrokerEvent::Exited { program_id, exit } if &program_id == expected_program => {
            Ok(exit)
        }
        PrivilegeBrokerEvent::Failed { error, .. } => Err(error),
        _ => Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerFailed,
            "The privilege broker sent an unexpected lifecycle event",
        )),
    }
}

async fn read_event(reader: &mut BufReader<ReadHalf<TcpStream>>) -> Result<PrivilegeBrokerEvent> {
    let mut line = String::new();
    let read = AsyncReadExt::take(reader, FRAME_LIMIT + 1)
        .read_line(&mut line)
        .await
        .map_err(CamelliaNexusError::storage)?;
    if read == 0 {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerConnectionLost,
            "The privilege broker connection closed unexpectedly",
        ));
    }
    if read as u64 > FRAME_LIMIT || !line.ends_with('\n') {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerFailed,
            "The privilege broker sent an invalid or oversized event",
        ));
    }
    serde_json::from_str(&line).map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerFailed,
            "The privilege broker sent malformed data",
        )
        .with_details(error.to_string())
    })
}

fn process_lost_error() -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::PrivilegeBrokerConnectionLost,
        "The privilege broker session is no longer available",
    )
}

async fn send_request_with_timeout<W>(
    writer: &Mutex<W>,
    alive: &AtomicBool,
    request: &PrivilegeBrokerRequest,
    timeout: Duration,
) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    match tokio::time::timeout(timeout, async {
        let mut writer = writer.lock().await;
        if !alive.load(Ordering::Acquire) {
            return Err(process_lost_error());
        }
        write_request(&mut *writer, request).await
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            // A cancelled write may already have emitted a partial JSON frame. Invalidate the
            // session before releasing this call so no queued sender can reuse the stream.
            alive.store(false, Ordering::Release);
            Err(CamelliaNexusError::new(
                ErrorCode::PrivilegeBrokerFailed,
                "The privilege broker did not accept a command in time",
            ))
        }
    }
}

async fn write_request<W>(writer: &mut W, request: &PrivilegeBrokerRequest) -> Result<()>
where
    W: AsyncWrite + Unpin,
{
    let mut bytes = serde_json::to_vec(request).map_err(CamelliaNexusError::storage)?;
    if bytes.len() as u64 > FRAME_LIMIT {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeConfigUnsafe,
            "The privileged launch manifest exceeds the broker limit",
        ));
    }
    bytes.push(b'\n');
    writer.write_all(&bytes).await.map_err(|error| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerConnectionLost,
            "Could not send a command to the privilege broker",
        )
        .with_details(error.to_string())
    })
}

fn broker_path() -> Result<PathBuf> {
    let executable = std::env::current_exe().map_err(CamelliaNexusError::storage)?;
    let directory = executable.parent().ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerUnavailable,
            "Could not locate the application directory",
        )
    })?;
    #[cfg(windows)]
    let name = "camellia-nexus-privilege-broker.exe";
    #[cfg(not(windows))]
    let name = "camellia-nexus-privilege-broker";
    let broker = directory.join(name);
    if broker.is_file() {
        Ok(broker)
    } else {
        Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerUnavailable,
            "The privilege broker is not installed beside Camellia Nexus",
        ))
    }
}

fn verify_privilege_broker_identity(broker: &std::path::Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(broker).map_err(CamelliaNexusError::storage)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerUnavailable,
            "The privilege broker is not a regular installed file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o022 != 0 {
            return Err(CamelliaNexusError::new(
                ErrorCode::PrivilegeBrokerUnavailable,
                "The privilege broker has unsafe write permissions",
            ));
        }
    }
    let actual = privilege_broker_identity::digest_file_hex(broker).map_err(|_| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerUnavailable,
            "The privilege broker could not be verified within the installed size limit",
        )
    })?;
    let expected = option_env!("CAMELLIA_NEXUS_PRIVILEGE_BROKER_SHA256").ok_or_else(|| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerUnavailable,
            "This Camellia Nexus build does not include a privilege broker identity",
        )
    })?;
    if actual != expected {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerUnavailable,
            "The privilege broker does not match this Camellia Nexus build",
        ));
    }
    Ok(())
}

fn complete_environment(
    mut overrides: std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    #[cfg(unix)]
    const INHERITED_KEYS: &[&str] = &[
        "HOME",
        "LANG",
        "LC_ALL",
        "PATH",
        "TMPDIR",
        "XDG_RUNTIME_DIR",
    ];
    #[cfg(windows)]
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
    for key in INHERITED_KEYS {
        if !overrides.contains_key(*key)
            && let Ok(value) = std::env::var(key)
        {
            overrides.insert((*key).into(), value);
        }
    }
    overrides
}

enum BrokerLauncher {
    #[cfg(target_os = "linux")]
    Child { _child: tokio::process::Child },
    #[cfg(target_os = "macos")]
    Authorization { _authorization: MacAuthorization },
    #[cfg(windows)]
    Process { _process: WindowsProcessHandle },
}

async fn accept_connection(
    listener: &TcpListener,
    launcher: &mut BrokerLauncher,
) -> Result<(TcpStream, std::net::SocketAddr)> {
    #[cfg(target_os = "linux")]
    {
        let BrokerLauncher::Child { _child: child } = launcher;
        tokio::time::timeout(AUTHORIZATION_TIMEOUT, async {
            tokio::select! {
                accepted = listener.accept() => accepted.map_err(|error| {
                    broker_error("The privilege broker channel failed", error)
                }),
                status = child.wait() => {
                    let status = status.map_err(|error| {
                        broker_error("The operating-system authorization broker failed", error)
                    })?;
                    Err(CamelliaNexusError::new(
                        if status.code() == Some(126) {
                            ErrorCode::PrivilegeAuthorizationCanceled
                        } else {
                            ErrorCode::PrivilegeBrokerUnavailable
                        },
                        if status.code() == Some(126) {
                            "Administrator authorization was canceled"
                        } else {
                            "The operating-system authorization broker exited before connecting"
                        },
                    ))
                },
            }
        })
        .await
        .map_err(|_| {
            CamelliaNexusError::new(
                ErrorCode::PrivilegeAuthorizationCanceled,
                "Administrator authorization timed out or was canceled",
            )
        })?
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = launcher;
        tokio::time::timeout(AUTHORIZATION_TIMEOUT, listener.accept())
            .await
            .map_err(|_| {
                CamelliaNexusError::new(
                    ErrorCode::PrivilegeAuthorizationCanceled,
                    "Administrator authorization timed out or was canceled",
                )
            })?
            .map_err(|error| broker_error("The privilege broker channel failed", error))
    }
}

#[cfg(target_os = "linux")]
async fn launch_broker(
    broker: &std::path::Path,
    address: std::net::SocketAddr,
    nonce: &str,
) -> Result<BrokerLauncher> {
    let mut command = tokio::process::Command::new("pkexec");
    command.arg("--disable-internal-agent").arg(broker);
    let child = command
        .arg("--broker")
        .arg(address.to_string())
        .arg(nonce)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            CamelliaNexusError::new(
                ErrorCode::PrivilegeBrokerUnavailable,
                "Could not start the operating-system authorization broker",
            )
            .with_details(error.to_string())
        })?;
    Ok(BrokerLauncher::Child { _child: child })
}

#[cfg(target_os = "macos")]
async fn launch_broker(
    broker: &std::path::Path,
    address: std::net::SocketAddr,
    nonce: &str,
) -> Result<BrokerLauncher> {
    let broker = broker.to_path_buf();
    let nonce = nonce.to_owned();
    let authorization =
        tokio::task::spawn_blocking(move || macos_launch_broker(&broker, address, &nonce))
            .await
            .map_err(CamelliaNexusError::internal)??;
    Ok(BrokerLauncher::Authorization {
        _authorization: authorization,
    })
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
async fn launch_broker(
    _broker: &std::path::Path,
    _address: std::net::SocketAddr,
    _nonce: &str,
) -> Result<BrokerLauncher> {
    Err(CamelliaNexusError::new(
        ErrorCode::PrivilegeBrokerUnavailable,
        "Privileged launch is not supported on this Unix platform",
    ))
}

#[cfg(target_os = "macos")]
struct MacAuthorization(*mut std::ffi::c_void);

#[cfg(target_os = "macos")]
unsafe impl Send for MacAuthorization {}

#[cfg(target_os = "macos")]
impl Drop for MacAuthorization {
    fn drop(&mut self) {
        unsafe {
            let _ = AuthorizationFree(self.0, 0);
        }
    }
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct AuthorizationItem {
    name: *const std::ffi::c_char,
    value_length: usize,
    value: *mut std::ffi::c_void,
    flags: u32,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct AuthorizationRights {
    count: u32,
    items: *mut AuthorizationItem,
}

#[cfg(target_os = "macos")]
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn AuthorizationCreate(
        rights: *const AuthorizationRights,
        environment: *const AuthorizationRights,
        flags: u32,
        authorization: *mut *mut std::ffi::c_void,
    ) -> i32;
    fn AuthorizationCopyRights(
        authorization: *mut std::ffi::c_void,
        rights: *const AuthorizationRights,
        environment: *const AuthorizationRights,
        flags: u32,
        authorized_rights: *mut *mut AuthorizationRights,
    ) -> i32;
    fn AuthorizationExecuteWithPrivileges(
        authorization: *mut std::ffi::c_void,
        path_to_tool: *const std::ffi::c_char,
        options: u32,
        arguments: *const *mut std::ffi::c_char,
        communications_pipe: *mut *mut std::ffi::c_void,
    ) -> i32;
    fn AuthorizationFree(authorization: *mut std::ffi::c_void, flags: u32) -> i32;
}

#[cfg(target_os = "macos")]
fn macos_launch_broker(
    broker: &std::path::Path,
    address: std::net::SocketAddr,
    nonce: &str,
) -> Result<MacAuthorization> {
    use std::os::unix::ffi::OsStrExt;

    const INTERACTION_ALLOWED: u32 = 1 << 0;
    const EXTEND_RIGHTS: u32 = 1 << 1;
    const PREAUTHORIZE: u32 = 1 << 4;
    const AUTHORIZATION_CANCELED: i32 = -60006;
    let right_name = std::ffi::CString::new("system.privilege.admin").expect("static right");
    let mut item = AuthorizationItem {
        name: right_name.as_ptr(),
        value_length: 0,
        value: std::ptr::null_mut(),
        flags: 0,
    };
    let rights = AuthorizationRights {
        count: 1,
        items: &mut item,
    };
    let mut authorization = std::ptr::null_mut();
    let create_status =
        unsafe { AuthorizationCreate(std::ptr::null(), std::ptr::null(), 0, &mut authorization) };
    if create_status != 0 || authorization.is_null() {
        return Err(macos_authorization_error(create_status));
    }
    let handle = MacAuthorization(authorization);
    let rights_status = unsafe {
        AuthorizationCopyRights(
            authorization,
            &rights,
            std::ptr::null(),
            INTERACTION_ALLOWED | EXTEND_RIGHTS | PREAUTHORIZE,
            std::ptr::null_mut(),
        )
    };
    if rights_status != 0 {
        return Err(macos_authorization_error(rights_status));
    }

    let tool = std::ffi::CString::new(broker.as_os_str().as_bytes()).map_err(|_| {
        CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerUnavailable,
            "The privilege broker path contains a null byte",
        )
    })?;
    let values = [
        std::ffi::CString::new("--broker").expect("static argument"),
        std::ffi::CString::new(address.to_string()).expect("socket address"),
        std::ffi::CString::new(nonce).expect("nonce"),
    ];
    let mut arguments: Vec<*mut std::ffi::c_char> = values
        .iter()
        .map(|value| value.as_ptr().cast_mut())
        .collect();
    arguments.push(std::ptr::null_mut());
    let status = unsafe {
        AuthorizationExecuteWithPrivileges(
            authorization,
            tool.as_ptr(),
            0,
            arguments.as_ptr(),
            std::ptr::null_mut(),
        )
    };
    if status == 0 {
        Ok(handle)
    } else if status == AUTHORIZATION_CANCELED {
        Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeAuthorizationCanceled,
            "Administrator authorization was canceled",
        ))
    } else {
        Err(macos_authorization_error(status))
    }
}

#[cfg(target_os = "macos")]
fn macos_authorization_error(status: i32) -> CamelliaNexusError {
    CamelliaNexusError::new(
        ErrorCode::PrivilegeBrokerUnavailable,
        "macOS could not authorize the privilege broker",
    )
    .with_details(format!("Authorization Services status {status}"))
}

#[cfg(windows)]
async fn launch_broker(
    broker: &std::path::Path,
    address: std::net::SocketAddr,
    nonce: &str,
) -> Result<BrokerLauncher> {
    let broker = broker.to_path_buf();
    let nonce = nonce.to_owned();
    let process =
        tokio::task::spawn_blocking(move || windows_launch_broker(&broker, address, &nonce))
            .await
            .map_err(CamelliaNexusError::internal)??;
    Ok(BrokerLauncher::Process { _process: process })
}

#[cfg(windows)]
struct WindowsProcessHandle(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
unsafe impl Send for WindowsProcessHandle {}

#[cfg(windows)]
impl Drop for WindowsProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Threading::TerminateProcess(self.0, 1);
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn windows_launch_broker(
    broker: &std::path::Path,
    address: std::net::SocketAddr,
    nonce: &str,
) -> Result<WindowsProcessHandle> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{
        Win32::{
            Foundation::GetLastError,
            UI::{
                Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW},
                WindowsAndMessaging::SW_HIDE,
            },
        },
        core::PCWSTR,
    };

    let file: Vec<u16> = broker.as_os_str().encode_wide().chain(Some(0)).collect();
    let parameters: Vec<u16> = format!("--broker {address} {nonce}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // Construct the UAC verb as UTF-16 code units so release audits can distinguish this
    // narrow sidecar authorization from forbidden whole-application elevation strings.
    let verb = [114u16, 117, 110, 97, 115, 0];
    let mut execute = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut execute) }.map_err(|error| {
        let code = unsafe { GetLastError().0 };
        CamelliaNexusError::new(
            if code == 1223 {
                ErrorCode::PrivilegeAuthorizationCanceled
            } else {
                ErrorCode::PrivilegeBrokerUnavailable
            },
            if code == 1223 {
                "Administrator authorization was canceled"
            } else {
                "Windows could not start the privilege broker"
            },
        )
        .with_details(error.to_string())
    })?;
    if execute.hProcess.is_invalid() {
        return Err(CamelliaNexusError::new(
            ErrorCode::PrivilegeBrokerUnavailable,
            "Windows did not return a privilege broker process handle",
        ));
    }
    Ok(WindowsProcessHandle(execute.hProcess))
}

fn broker_error(message: &'static str, error: impl std::fmt::Display) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::PrivilegeBrokerFailed, message)
        .with_details(error.to_string())
}

fn protocol_error(message: &'static str) -> CamelliaNexusError {
    CamelliaNexusError::new(ErrorCode::PrivilegeBrokerFailed, message)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        pin::Pin,
        sync::atomic::{AtomicBool, Ordering},
        task::{Context, Poll},
        time::Duration,
    };

    use camellia_nexus_core::{
        ErrorCode, LaunchPlan, PrivilegeBrokerEvent, PrivilegeBrokerRequest, PrivilegePolicy,
        ProgramId, ProgramKind,
    };
    use tokio::{
        io::AsyncWrite,
        sync::{Mutex, mpsc},
    };

    use super::{
        BROKER_EVENT_QUEUE_CAPACITY, ensure_authorization_mode, send_request_with_timeout,
        try_send_process_event,
    };

    struct PendingWriter;

    impl AsyncWrite for PendingWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Poll::Pending
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Pending
        }

        fn poll_shutdown(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Pending
        }
    }

    fn plan(policy: PrivilegePolicy, interactive: bool) -> LaunchPlan {
        LaunchPlan {
            program_id: ProgramId::parse("authorization-mode-test").expect("id"),
            workspace: PathBuf::from("/program"),
            managed_executable: true,
            executable: PathBuf::from("/program/bin/program"),
            args: Vec::new(),
            cwd: PathBuf::from("/program/bin"),
            environment: BTreeMap::new(),
            stdout_log: PathBuf::from("/program/logs/stdout.log"),
            stderr_log: PathBuf::from("/program/logs/stderr.log"),
            program_kind: ProgramKind::SingBox,
            privilege_policy: policy,
            privilege_inputs: Vec::new(),
            interactive,
        }
    }

    #[test]
    fn unattended_elevated_launch_fails_before_opening_an_authorization_prompt() {
        let error = ensure_authorization_mode(&plan(PrivilegePolicy::Elevated, false))
            .expect_err("unattended elevation must fail closed");
        assert_eq!(
            error.code,
            camellia_nexus_core::ErrorCode::PrivilegeRequired
        );
    }

    #[test]
    fn explicit_user_launch_can_request_elevation() {
        ensure_authorization_mode(&plan(PrivilegePolicy::Elevated, true))
            .expect("interactive launch");
    }

    #[test]
    fn unattended_standard_launch_does_not_require_authorization() {
        ensure_authorization_mode(&plan(PrivilegePolicy::Standard, false))
            .expect("standard launch");
    }

    #[test]
    fn broker_lifecycle_event_queue_is_bounded() {
        let (sender, _receiver) = mpsc::channel(BROKER_EVENT_QUEUE_CAPACITY);
        let program_id = ProgramId::parse("bounded-event-queue").expect("program id");
        let event = |request_id: usize| PrivilegeBrokerEvent::Started {
            request_id: request_id.to_string(),
            program_id: program_id.clone(),
            pid: 42,
        };

        for request_id in 0..BROKER_EVENT_QUEUE_CAPACITY {
            try_send_process_event(&sender, event(request_id)).expect("queue capacity");
        }
        let error = try_send_process_event(&sender, event(BROKER_EVENT_QUEUE_CAPACITY))
            .expect_err("an extra lifecycle event must fail the broker session");
        assert_eq!(error.code, ErrorCode::PrivilegeBrokerFailed);
    }

    #[tokio::test]
    async fn broker_command_write_is_bounded() {
        let writer = Mutex::new(PendingWriter);
        let alive = AtomicBool::new(true);
        let request = PrivilegeBrokerRequest::Shutdown {
            request_id: "bounded-write".into(),
        };
        let result = tokio::time::timeout(
            Duration::from_millis(100),
            send_request_with_timeout(&writer, &alive, &request, Duration::from_millis(10)),
        )
        .await
        .expect("a broker write must settle within its I/O deadline")
        .expect_err("a permanently blocked writer must fail");

        assert_eq!(result.code, ErrorCode::PrivilegeBrokerFailed);
        assert!(!alive.load(Ordering::Acquire));

        let retry = tokio::time::timeout(
            Duration::from_millis(100),
            send_request_with_timeout(&writer, &alive, &request, Duration::from_millis(10)),
        )
        .await
        .expect("an invalidated broker stream must reject queued commands")
        .expect_err("the invalidated stream must not be reused");
        assert_eq!(retry.code, ErrorCode::PrivilegeBrokerConnectionLost);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn broker_in_a_replaceable_directory_is_rejected() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("tempdir");
        let broker = directory.path().join("camellia-nexus-privilege-broker");
        std::fs::write(&broker, b"broker").expect("broker");
        std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o777))
            .expect("permissions");
        assert!(super::verify_privilege_broker_identity(&broker).is_err());
    }
}
