//! What this machine can actually run a job in. The owner picks a backend, the
//! shell asks the machine whether it has one, and the node is told the answer.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;

use crate::settings::ComputeBackend;

/// A probe answers from a local daemon or not at all; a slow answer is treated
/// as no answer, because the node start waits on it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Where a delegated cgroup v2 tree is mounted.
const CGROUP_MOUNT: &str = "/sys/fs/cgroup";

/// Controllers apptainer needs in the delegated slice to hold a job to its
/// limits.
const NEEDED: [&str; 2] = ["memory", "pids"];

/// The executor the node is told to build. The values are what the node's
/// `ARUNA_COMPUTE_EXECUTOR` accepts.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Executor {
    Docker,
    Apptainer,
}

impl Executor {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Apptainer => "apptainer",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ComputeProbe {
    pub docker: DockerProbe,
    pub apptainer: ApptainerProbe,
}

/// `available` means the daemon answered, not merely that a client is
/// installed: the version asked for is the server's.
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DockerProbe {
    pub available: bool,
    pub version: Option<String>,
    pub socket: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApptainerProbe {
    pub available: bool,
    pub version: Option<String>,
    /// Whether this session owns a delegated cgroup slice apptainer can put a
    /// job into. Without one the node still runs jobs, but without limits.
    pub cgroup_ok: bool,
    pub cgroup_root: Option<String>,
}

/// What the shell tells the node to run once the machine has answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComputePlan {
    pub executor: Option<Executor>,
    pub cgroup_root: Option<String>,
}

/// Asks both backends what they are, in parallel. A missing binary is an
/// unavailable backend, never a failure.
pub async fn probe() -> ComputeProbe {
    let (docker, apptainer) = tokio::join!(docker_probe(), apptainer_probe());
    ComputeProbe { docker, apptainer }
}

/// The owner's choice against what the machine has. `Auto` prefers docker
/// because it needs no delegated cgroup; an explicit choice is honoured even
/// when the probe found nothing, so the node reports the real reason.
pub fn resolve(backend: ComputeBackend, probe: &ComputeProbe) -> Option<Executor> {
    match backend {
        ComputeBackend::Off => None,
        ComputeBackend::Docker => Some(Executor::Docker),
        ComputeBackend::Apptainer => Some(Executor::Apptainer),
        ComputeBackend::Auto if probe.docker.available => Some(Executor::Docker),
        ComputeBackend::Auto if probe.apptainer.available => Some(Executor::Apptainer),
        ComputeBackend::Auto => None,
    }
}

pub fn plan(backend: ComputeBackend, probe: &ComputeProbe) -> ComputePlan {
    let executor = resolve(backend, probe);
    ComputePlan {
        cgroup_root: match executor {
            Some(Executor::Apptainer) => probe.apptainer.cgroup_root.clone(),
            _ => None,
        },
        executor,
    }
}

/// The plan for a node start. Nothing is probed while local compute is off.
pub async fn detect(backend: ComputeBackend) -> ComputePlan {
    match backend {
        ComputeBackend::Off => ComputePlan::default(),
        _ => plan(backend, &probe().await),
    }
}

async fn docker_probe() -> DockerProbe {
    let version = run("docker", &["version", "--format", "{{.Server.Version}}"])
        .await
        .as_deref()
        .and_then(version);
    DockerProbe {
        available: version.is_some(),
        version,
        socket: docker_socket(),
    }
}

async fn apptainer_probe() -> ApptainerProbe {
    let version = run("apptainer", &["version"])
        .await
        .as_deref()
        .and_then(version);
    let cgroup_root = user_slice().map(|path| path.display().to_string());
    ApptainerProbe {
        available: version.is_some(),
        version,
        cgroup_ok: cgroup_root.is_some(),
        cgroup_root,
    }
}

/// Runs a probe command off the runtime's threads. Anything but a clean exit
/// within the timeout answers `None`, including a binary that is not installed.
async fn run(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(PROBE_TIMEOUT, output).await {
        Ok(Ok(output)) if output.status.success() => {
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        _ => None,
    }
}

/// The first non-empty line, without the prefix the `apptainer version` output
/// carries on some builds.
fn version(raw: &str) -> Option<String> {
    raw.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| {
            line.trim_start_matches("apptainer version")
                .trim()
                .to_string()
        })
        .filter(|version| !version.is_empty())
}

/// Where the daemon listens, for the owner to see which docker answered.
fn docker_socket() -> Option<String> {
    if let Some(host) = std::env::var_os("DOCKER_HOST").filter(|host| !host.is_empty()) {
        return Some(host.to_string_lossy().into_owned());
    }
    let rootless = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .map(|dir| dir.join("docker.sock"));
    [rootless, Some(PathBuf::from("/var/run/docker.sock"))]
        .into_iter()
        .flatten()
        .find(|path| path.exists())
        .map(|path| format!("unix://{}", path.display()))
}

/// The delegated cgroup slice of this login session, when systemd handed one
/// over with the controllers a job needs.
fn user_slice() -> Option<PathBuf> {
    let path = slice(&std::fs::read_to_string("/proc/self/cgroup").ok()?)?;
    let root = Path::new(CGROUP_MOUNT).join(path.trim_start_matches('/'));
    let controllers = std::fs::read_to_string(root.join("cgroup.controllers")).ok()?;
    delegated(&controllers).then_some(root)
}

/// The `user@<uid>.service` prefix of a cgroup v2 path, which is the slice
/// systemd delegates to a login session.
fn slice(raw: &str) -> Option<String> {
    let path = raw
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?
        .trim();
    let mut walked = String::new();
    for part in path.split('/').skip(1) {
        walked.push('/');
        walked.push_str(part);
        if part.starts_with("user@") && part.ends_with(".service") {
            return Some(walked);
        }
    }
    None
}

fn delegated(controllers: &str) -> bool {
    NEEDED
        .iter()
        .all(|needed| controllers.split_whitespace().any(|have| have == *needed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(docker: bool, apptainer: bool) -> ComputeProbe {
        ComputeProbe {
            docker: DockerProbe {
                available: docker,
                ..DockerProbe::default()
            },
            apptainer: ApptainerProbe {
                available: apptainer,
                cgroup_ok: true,
                cgroup_root: Some("/sys/fs/cgroup/user.slice/user@1000.service".into()),
                ..ApptainerProbe::default()
            },
        }
    }

    #[test]
    fn reads_versions() {
        assert_eq!(version("27.3.1\n").as_deref(), Some("27.3.1"));
        assert_eq!(
            version("apptainer version 1.3.4\n").as_deref(),
            Some("1.3.4")
        );
        assert_eq!(version("1.3.4\nextra\n").as_deref(), Some("1.3.4"));
        assert!(version("\n  \n").is_none());
        assert!(version("").is_none());
    }

    #[test]
    fn prefers_docker_first() {
        // Auto takes the daemon that answers; docker needs no delegated slice.
        assert_eq!(
            resolve(ComputeBackend::Auto, &seen(true, true)),
            Some(Executor::Docker)
        );
        assert_eq!(
            resolve(ComputeBackend::Auto, &seen(false, true)),
            Some(Executor::Apptainer)
        );
        assert!(resolve(ComputeBackend::Auto, &seen(false, false)).is_none());
        assert!(resolve(ComputeBackend::Off, &seen(true, true)).is_none());
    }

    #[test]
    fn honours_an_explicit_choice() {
        // The node reports why a chosen backend did not build; guessing another
        // one behind the owner's back would hide that.
        assert_eq!(
            resolve(ComputeBackend::Apptainer, &seen(true, false)),
            Some(Executor::Apptainer)
        );
        assert_eq!(
            resolve(ComputeBackend::Docker, &seen(false, true)),
            Some(Executor::Docker)
        );
    }

    #[test]
    fn carries_the_slice() {
        // Only apptainer is put into a cgroup; docker's daemon owns its own.
        assert_eq!(
            plan(ComputeBackend::Apptainer, &seen(false, true))
                .cgroup_root
                .as_deref(),
            Some("/sys/fs/cgroup/user.slice/user@1000.service")
        );
        assert!(
            plan(ComputeBackend::Docker, &seen(true, true))
                .cgroup_root
                .is_none()
        );
    }

    #[test]
    fn finds_the_user_slice() {
        assert_eq!(
            slice("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app.scope\n")
                .as_deref(),
            Some("/user.slice/user-1000.slice/user@1000.service")
        );
        // A cgroup v1 listing and a system service both name no user slice.
        assert!(slice("12:pids:/user.slice\n11:memory:/user.slice\n").is_none());
        assert!(slice("0::/system.slice/aruna.service\n").is_none());
    }

    #[test]
    fn wants_both_controllers() {
        assert!(delegated("cpu io memory pids"));
        assert!(!delegated("cpu io memory"));
        assert!(!delegated(""));
    }
}
