mod binary;
pub mod compute;
mod health;
mod logs;

use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;

use crate::error::DesktopError;
use crate::settings::NodeSettings;

pub use health::{NodeInfo, info, probe, ready, realm_id};
pub use logs::{LogBuffer, Stream};

/// Emitted to the frontend whenever the supervised node changes state.
pub const STATUS_EVENT: &str = "node-status";

/// A node that keeps dying is a configuration problem, not a transient one.
const MAX_RESTARTS: u32 = 5;
const RESTART_BACKOFF: Duration = Duration::from_secs(2);

/// How often a started node is asked whether it serves yet. A join may take a
/// while, so this waits for as long as the child lives.
const READY_POLL: Duration = Duration::from_millis(500);

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
    /// Whether the node answers on its ops listener. A spawned node is not one
    /// yet: it binds nothing until its join and startup are done.
    pub ready: bool,
    pub pid: Option<u32>,
    pub binary: Option<String>,
    pub detail: Option<String>,
    pub restarts: u32,
    pub last_exit: Option<i32>,
}

#[derive(Default)]
struct Inner {
    status: NodeStatus,
    /// When the current run reached `Running`, for the uptime the portal shows.
    started: Option<std::time::Instant>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<tauri::async_runtime::JoinHandle<()>>,
    /// Which start owns the run slot, so an older run loop ending cannot free
    /// a slot a newer start already claimed.
    generation: u64,
}

/// Owns the node child process: one run loop per start, restarts on unexpected
/// exits, and a SIGTERM-first stop that honours the node's own grace period.
#[derive(Clone)]
pub struct Supervisor {
    app: AppHandle,
    data_dir: PathBuf,
    resources: Option<PathBuf>,
    settings: Arc<Mutex<NodeSettings>>,
    /// Only ever used against the node's own loopback listeners.
    http: reqwest::Client,
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
            http: reqwest::Client::new(),
            logs: LogBuffer::default(),
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn logs(&self) -> &LogBuffer {
        &self.logs
    }

    pub fn status(&self) -> NodeStatus {
        self.inner
            .lock()
            .expect("supervisor poisoned")
            .status
            .clone()
    }

    /// Starts the run loop, which probes this machine and writes the node's env
    /// file before spawning it. The run slot is claimed first, so a second
    /// start finds it taken instead of supervising a second node.
    pub fn start(&self) -> Result<NodeStatus, DesktopError> {
        let (sender, receiver) = oneshot::channel();
        let generation = {
            let mut inner = self.inner.lock().expect("supervisor poisoned");
            if inner.task.is_some() || inner.stop.is_some() {
                return Ok(inner.status.clone());
            }
            inner.generation += 1;
            inner.stop = Some(sender);
            inner.generation
        };

        let settings = self.settings.lock().expect("settings poisoned").clone();
        if let Err(error) = settings.validate() {
            self.release(generation);
            return Err(error);
        }

        let binary = match binary::resolve(&settings, self.resources.as_deref()) {
            Ok(path) => path,
            Err(error) => {
                self.release(generation);
                self.publish(NodeStatus {
                    state: NodeState::BinaryMissing,
                    detail: Some(error.to_string()),
                    ..NodeStatus::default()
                });
                return Err(error);
            }
        };

        self.logs.clear();
        let status = NodeStatus {
            state: NodeState::Starting,
            binary: Some(binary.display().to_string()),
            ..NodeStatus::default()
        };
        self.publish(status.clone());

        let supervisor = self.clone();
        let task = tauri::async_runtime::spawn(async move {
            supervisor
                .supervise(binary, settings, generation, receiver)
                .await;
        });
        self.inner.lock().expect("supervisor poisoned").task = Some(task);
        Ok(status)
    }

    /// Frees the run slot, unless a later start already claimed it.
    fn release(&self, generation: u64) {
        let mut inner = self.inner.lock().expect("supervisor poisoned");
        if inner.generation == generation {
            inner.stop = None;
            inner.task = None;
        }
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
        generation: u64,
        stop: oneshot::Receiver<()>,
    ) {
        self.run(&binary, &settings, stop).await;
        // However the loop ended, the next start has to be able to claim it.
        self.release(generation);
    }

    async fn run(&self, binary: &Path, settings: &NodeSettings, mut stop: oneshot::Receiver<()>) {
        // A stop arriving while the machine is probed must not be answered with
        // a node: the probe waits on daemons that may take their time.
        let plan = tokio::select! {
            plan = compute::detect(settings.compute.backend) => plan,
            _ = &mut stop => {
                self.publish(NodeStatus {
                    state: NodeState::Stopped,
                    binary: Some(binary.display().to_string()),
                    ..NodeStatus::default()
                });
                return;
            }
        };
        if let Some(note) = plan.note {
            self.logs.push(Stream::Desktop, note.to_string());
        }
        let env_path = match settings.write_env(&self.data_dir, &plan) {
            Ok(path) => path,
            Err(error) => return self.fail(binary, error.to_string(), 0),
        };
        self.logs.push(
            Stream::Desktop,
            format!("starting {} with {}", binary.display(), env_path.display()),
        );

        let grace = Duration::from_secs(settings.shutdown_grace_secs);
        let mut restarts = 0;

        loop {
            let mut child = match self.spawn_node(binary, settings, &plan) {
                Ok(child) => child,
                Err(error) => {
                    self.fail(binary, error.to_string(), restarts);
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

            let exit = self.watch(&mut child, &mut stop, settings.ports.ops).await;

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
                    binary,
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

    /// Waits for the child to exit or a stop to arrive, asking the node whether
    /// it serves yet in the meantime. `None` means the stop won.
    async fn watch(
        &self,
        child: &mut Child,
        stop: &mut oneshot::Receiver<()>,
        ops: u16,
    ) -> Option<std::io::Result<ExitStatus>> {
        let serving = self.await_ready(ops);
        tokio::pin!(serving);
        let mut waiting = true;
        loop {
            tokio::select! {
                exit = child.wait() => return Some(exit),
                _ = &mut *stop => return None,
                () = &mut serving, if waiting => {
                    waiting = false;
                    self.mark_ready();
                }
            }
        }
    }

    /// Answers once the node's readiness endpoint does. A join can take a while
    /// and has no deadline of ours: the child exiting ends the wait.
    async fn await_ready(&self, ops: u16) {
        while !ready(&self.http, ops).await {
            tokio::time::sleep(READY_POLL).await;
        }
    }

    /// Republishes the current run as serving, which is what the shell context
    /// and the portal's device view switch on.
    fn mark_ready(&self) {
        let status = self.status();
        if status.state == NodeState::Running && !status.ready {
            self.publish(NodeStatus {
                ready: true,
                ..status
            });
        }
    }

    fn spawn_node(
        &self,
        binary: &Path,
        settings: &NodeSettings,
        plan: &compute::ComputePlan,
    ) -> Result<Child, DesktopError> {
        let mut command = Command::new(binary);
        command
            .current_dir(&self.data_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // The compute profile is written key by key, so an inherited one would
        // survive in the keys this start does not write: drop them all first.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("ARUNA_COMPUTE_") {
                command.env_remove(key);
            }
        }
        // Set explicitly as well as in the env file: dotenvy never overrides an
        // inherited value, so a stale desktop environment must not leak in.
        for (key, value) in settings.env_pairs(&self.data_dir, plan) {
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

    /// Seconds the node has been running, `None` while it is not.
    pub fn uptime(&self) -> Option<u64> {
        self.inner
            .lock()
            .expect("supervisor poisoned")
            .started
            .map(|started| started.elapsed().as_secs())
    }

    fn publish(&self, status: NodeStatus) {
        let mut inner = self.inner.lock().expect("supervisor poisoned");
        inner.started = match status.state {
            NodeState::Running => inner.started.or_else(|| Some(std::time::Instant::now())),
            _ => None,
        };
        let flipped = flips(&inner.status, &status);
        inner.status = status.clone();
        drop(inner);
        if let Err(error) = self.app.emit(STATUS_EVENT, status) {
            tracing::warn!(%error, "Could not emit the node status");
        }
        // Only after this state is published, so the context the portal is told
        // about is the one it reads back.
        if flipped {
            crate::shell::sync(&self.app);
        }
    }
}

/// Whether a transition crosses "serving", which is the half of the shell
/// context the node owns: an enrolled node that answers is what the portal
/// talks to, and anything else sends it back to the realm. A spawned node is
/// not one yet, so the context must not follow the state alone.
fn flips(before: &NodeStatus, after: &NodeStatus) -> bool {
    before.ready != after.ready
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

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: NodeState, ready: bool) -> NodeStatus {
        NodeStatus {
            state,
            ready,
            ..NodeStatus::default()
        }
    }

    #[test]
    fn flips_on_serving() {
        // Only a node that answers moves the shell context: a spawned one binds
        // nothing yet, and every way out of serving takes the portal back.
        let spawned = status(NodeState::Running, false);
        let serving = status(NodeState::Running, true);
        assert!(!flips(&status(NodeState::Stopped, false), &spawned));
        assert!(flips(&spawned, &serving));
        assert!(flips(&serving, &status(NodeState::Stopping, false)));
        assert!(flips(&serving, &status(NodeState::Restarting, false)));
        assert!(flips(&serving, &status(NodeState::Failed, false)));
        assert!(!flips(&spawned, &status(NodeState::Restarting, false)));
        assert!(!flips(&status(NodeState::Starting, false), &spawned));
        assert!(!flips(&serving, &serving.clone()));
    }
}
