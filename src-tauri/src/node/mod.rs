mod binary;
mod health;
mod logs;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;

use crate::error::DesktopError;
use crate::settings::NodeSettings;

pub use health::{HealthReport, probe};
pub use logs::{LogBuffer, LogPage, Stream};

/// Emitted to the frontend whenever the supervised node changes state.
pub const STATUS_EVENT: &str = "node-status";

/// A node that keeps dying is a configuration problem, not a transient one.
const MAX_RESTARTS: u32 = 5;
const RESTART_BACKOFF: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NodeState {
    #[default]
    Stopped,
    Starting,
    Running,
    Stopping,
    Restarting,
    Failed,
    BinaryMissing,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeStatus {
    pub state: NodeState,
    pub pid: Option<u32>,
    pub binary: Option<String>,
    pub detail: Option<String>,
    pub restarts: u32,
    pub last_exit: Option<i32>,
}

#[derive(Default)]
struct Inner {
    status: NodeStatus,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
}

/// Owns the node child process: one run loop per start, restarts on unexpected
/// exits, and a SIGTERM-first stop that honours the node's own grace period.
#[derive(Clone)]
pub struct Supervisor {
    app: AppHandle,
    data_dir: PathBuf,
    resources: Option<PathBuf>,
    settings: Arc<Mutex<NodeSettings>>,
    logs: LogBuffer,
    inner: Arc<Mutex<Inner>>,
}

impl Supervisor {
    pub fn new(
        app: AppHandle,
        data_dir: PathBuf,
        resources: Option<PathBuf>,
        settings: Arc<Mutex<NodeSettings>>,
    ) -> Self {
        Self {
            app,
            data_dir,
            resources,
            settings,
            logs: LogBuffer::default(),
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn logs(&self) -> &LogBuffer {
        &self.logs
    }

    pub fn status(&self) -> NodeStatus {
        self.inner.lock().expect("supervisor poisoned").status.clone()
    }

    /// Resolves the binary, writes the node's env file and starts the run loop.
    /// A missing binary leaves a status the status view can explain.
    pub fn start(&self) -> Result<NodeStatus, DesktopError> {
        if self.inner.lock().expect("supervisor poisoned").task.is_some() {
            return Ok(self.status());
        }

        let settings = self.settings.lock().expect("settings poisoned").clone();
        settings.validate()?;

        let binary = match binary::resolve(&settings, self.resources.as_deref()) {
            Ok(path) => path,
            Err(error) => {
                self.publish(NodeStatus {
                    state: NodeState::BinaryMissing,
                    detail: Some(error.to_string()),
                    ..NodeStatus::default()
                });
                return Err(error);
            }
        };

        let env_path = settings.write_env(&self.data_dir)?;
        self.logs.clear();
        self.logs.push(
            Stream::Desktop,
            format!("starting {} with {}", binary.display(), env_path.display()),
        );

        let (sender, receiver) = oneshot::channel();
        let status = NodeStatus {
            state: NodeState::Starting,
            binary: Some(binary.display().to_string()),
            ..NodeStatus::default()
        };
        self.publish(status.clone());

        let supervisor = self.clone();
        let task = tauri::async_runtime::spawn(async move {
            supervisor.supervise(binary, settings, receiver).await;
        });

        let mut inner = self.inner.lock().expect("supervisor poisoned");
        inner.stop = Some(sender);
        inner.task = Some(task);
        Ok(status)
    }

    /// Signals the run loop and waits for the child to be gone.
    pub async fn stop(&self) -> NodeStatus {
        let (sender, task) = {
            let mut inner = self.inner.lock().expect("supervisor poisoned");
            (inner.stop.take(), inner.task.take())
        };
        if let Some(sender) = sender {
            let _ = sender.send(());
        }
        if let Some(task) = task {
            let _ = task.await;
        }
        self.status()
    }

    pub async fn restart(&self) -> Result<NodeStatus, DesktopError> {
        self.stop().await;
        self.start()
    }

    async fn supervise(
        self,
        binary: PathBuf,
        settings: NodeSettings,
        mut stop: oneshot::Receiver<()>,
    ) {
        let grace = Duration::from_secs(settings.shutdown_grace_secs);
        let mut restarts = 0;

        loop {
            let mut child = match self.spawn_node(&binary, &settings) {
                Ok(child) => child,
                Err(error) => {
                    self.fail(&binary, error.to_string(), restarts);
                    return;
                }
            };
            let pid = child.id();
            self.tail(&mut child);
            self.publish(NodeStatus {
                state: NodeState::Running,
                pid,
                binary: Some(binary.display().to_string()),
                restarts,
                ..NodeStatus::default()
            });

            let exit = tokio::select! {
                exit = child.wait() => Some(exit),
                _ = &mut stop => None,
            };

            let Some(exit) = exit else {
                self.publish(NodeStatus {
                    state: NodeState::Stopping,
                    pid,
                    binary: Some(binary.display().to_string()),
                    restarts,
                    ..NodeStatus::default()
                });
                self.stop_child(&mut child, grace).await;
                self.publish(NodeStatus {
                    state: NodeState::Stopped,
                    binary: Some(binary.display().to_string()),
                    restarts,
                    ..NodeStatus::default()
                });
                return;
            };

            let code = exit.ok().and_then(|status| status.code());
            self.logs.push(
                Stream::Desktop,
                format!("node exited with {}", describe_exit(code)),
            );
            restarts += 1;
            if restarts > MAX_RESTARTS {
                self.fail(
                    &binary,
                    format!("gave up after {MAX_RESTARTS} restarts"),
                    restarts - 1,
                );
                return;
            }

            self.publish(NodeStatus {
                state: NodeState::Restarting,
                binary: Some(binary.display().to_string()),
                detail: Some(format!("exited with {}", describe_exit(code))),
                restarts,
                last_exit: code,
                ..NodeStatus::default()
            });

            let waited = tokio::time::timeout(RESTART_BACKOFF, &mut stop).await;
            if waited.is_ok() {
                self.publish(NodeStatus {
                    state: NodeState::Stopped,
                    binary: Some(binary.display().to_string()),
                    restarts,
                    last_exit: code,
                    ..NodeStatus::default()
                });
                return;
            }
        }
    }

    fn spawn_node(&self, binary: &Path, settings: &NodeSettings) -> Result<Child, DesktopError> {
        let mut command = Command::new(binary);
        command
            .current_dir(&self.data_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Set explicitly as well as in the env file: dotenvy never overrides an
        // inherited value, so a stale desktop environment must not leak in.
        for (key, value) in settings.env_pairs(&self.data_dir) {
            command.env(key, value);
        }
        command
            .spawn()
            .map_err(|error| DesktopError::Spawn(error.to_string()))
    }

    fn tail(&self, child: &mut Child) {
        if let Some(stdout) = child.stdout.take() {
            self.pump(Stream::Stdout, stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            self.pump(Stream::Stderr, stderr);
        }
    }

    fn pump<R: AsyncRead + Unpin + Send + 'static>(&self, stream: Stream, reader: R) {
        let logs = self.logs.clone();
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                logs.push(stream, line);
            }
        });
    }

    /// SIGTERM first so the node runs its own graceful shutdown, then a kill
    /// once its grace period has passed.
    async fn stop_child(&self, child: &mut Child, grace: Duration) {
        if let Some(pid) = child.id()
            && signal_term(pid)
            && tokio::time::timeout(grace, child.wait()).await.is_ok()
        {
            return;
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
    }

    fn fail(&self, binary: &Path, detail: String, restarts: u32) {
        self.logs.push(Stream::Desktop, detail.clone());
        self.publish(NodeStatus {
            state: NodeState::Failed,
            binary: Some(binary.display().to_string()),
            detail: Some(detail),
            restarts,
            ..NodeStatus::default()
        });
    }

    fn publish(&self, status: NodeStatus) {
        self.inner.lock().expect("supervisor poisoned").status = status.clone();
        if let Err(error) = self.app.emit(STATUS_EVENT, status) {
            tracing::warn!(%error, "Could not emit the node status");
        }
    }
}

fn describe_exit(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("code {code}"),
        None => "a signal".to_string(),
    }
}

#[cfg(unix)]
fn signal_term(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    let Some(pid) = rustix::process::Pid::from_raw(raw) else {
        return false;
    };
    match rustix::process::kill_process(pid, rustix::process::Signal::TERM) {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(%error, "Could not signal the node");
            false
        }
    }
}

/// Windows has no SIGTERM, so the caller falls through to the kill path.
#[cfg(not(unix))]
fn signal_term(_pid: u32) -> bool {
    false
}
