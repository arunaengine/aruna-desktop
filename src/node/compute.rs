//! What this machine can actually run a job in. The owner picks a backend, the
//! shell asks the machine whether it has one, and the node is told the answer.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;

use crate::settings::ComputeBackend;

/// A probe answers from a local daemon or not at all; a slow answer is treated
/// as no answer, because the node start waits on it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Keeps a probe from flashing a console window on Windows.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
    /// job into. The node refuses to build the backend without one.
    pub cgroup_ok: bool,
    pub cgroup_root: Option<String>,
}

/// What the shell tells the node to run once the machine has answered.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComputePlan {
    pub executor: Option<Executor>,
    pub cgroup_root: Option<String>,
    /// The daemon address found on this machine, handed to docker clients.
    pub docker_host: Option<String>,
    /// Why the owner's choice was dropped, for the desktop log.
    pub note: Option<&'static str>,
}

const NO_SLICE: &str = "apptainer needs a delegated user cgroup slice; local compute stays off";
const NO_BACKEND: &str = "no local container backend answered; local compute stays off";

/// Asks both backends what they are, in parallel. A missing binary is an
/// unavailable backend, never a failure.
pub async fn probe() -> ComputeProbe {
    let (docker, apptainer) = tokio::join!(docker_probe(), apptainer_probe());
    ComputeProbe { docker, apptainer }
}

/// The owner's choice against what the machine has. Apptainer is only ever
/// chosen with a delegated slice, because the node refuses to build it without
/// one; a chosen docker is honoured whatever the probe found, so the node
/// reports the real reason instead of the shell picking something else.
pub fn resolve(backend: ComputeBackend, probe: &ComputeProbe) -> Option<Executor> {
    let slice = probe.apptainer.cgroup_ok;
    match backend {
        ComputeBackend::Off => None,
        ComputeBackend::Docker => Some(Executor::Docker),
        ComputeBackend::Apptainer => slice.then_some(Executor::Apptainer),
        ComputeBackend::Auto if probe.docker.available => Some(Executor::Docker),
        ComputeBackend::Auto if probe.apptainer.available && slice => Some(Executor::Apptainer),
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
        docker_host: match executor {
            Some(Executor::Docker) => probe.docker.socket.clone(),
            _ => None,
        },
        note: match (backend, executor) {
            (ComputeBackend::Apptainer, None) => Some(NO_SLICE),
            (ComputeBackend::Auto, None) => Some(NO_BACKEND),
            _ => None,
        },
        executor,
    }
}

/// The plan for a node start. Only what the choice can still turn on is asked:
/// a chosen docker is written whatever answers, and compute that is off asks
/// nothing at all.
pub async fn detect(backend: ComputeBackend) -> ComputePlan {
    let seen = match backend {
        ComputeBackend::Off => ComputeProbe::default(),
        ComputeBackend::Docker => ComputeProbe {
            docker: DockerProbe {
                socket: docker_socket(),
                ..DockerProbe::default()
            },
            ..ComputeProbe::default()
        },
        ComputeBackend::Apptainer => ComputeProbe {
            apptainer: apptainer_probe().await,
            ..ComputeProbe::default()
        },
        ComputeBackend::Auto => probe().await,
    };
    plan(backend, &seen)
}

async fn docker_probe() -> DockerProbe {
    let socket = docker_socket();
    let version = run(
        "docker",
        &["version", "--format", "{{.Server.Version}}"],
        socket.as_deref(),
    )
    .await
    .as_deref()
    .and_then(version);
    DockerProbe {
        available: version.is_some(),
        version,
        socket,
    }
}

async fn apptainer_probe() -> ApptainerProbe {
    let version = run("apptainer", &["version"], None)
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
async fn run(program: &str, args: &[&str], docker_host: Option<&str>) -> Option<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("PATH", tool_path())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    if let Some(host) = docker_host {
        command.env("DOCKER_HOST", host);
    }
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = command.output();
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
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    socket_paths(runtime.as_deref(), home.as_deref())
        .into_iter()
        .find(|path| path.exists())
        .map(|path| format!("unix://{}", path.display()))
}

/// Where a docker daemon commonly listens: rootless and system installs, then
/// the sockets of Docker Desktop, Colima and OrbStack in the owner's home.
fn socket_paths(runtime: Option<&Path>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = runtime
        .map(|dir| dir.join("docker.sock"))
        .into_iter()
        .collect();
    paths.push(PathBuf::from("/var/run/docker.sock"));
    if let Some(home) = home {
        for relative in [
            ".docker/run/docker.sock",
            ".colima/default/docker.sock",
            ".orbstack/run/docker.sock",
        ] {
            paths.push(home.join(relative));
        }
    }
    paths
}

/// The tools' search path with the usual install places added. An app started
/// from Finder or a launcher gets a minimal one that misses them.
pub fn tool_path() -> OsString {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    extend_path(std::env::var_os("PATH"), home.as_deref())
}

fn extend_path(path: Option<OsString>, home: Option<&Path>) -> OsString {
    let mut dirs: Vec<PathBuf> = path
        .as_deref()
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    let mut usual = vec![
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
    ];
    usual.extend(home.map(|home| home.join(".docker/bin")));
    for dir in usual {
        if dir.is_dir() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    std::env::join_paths(dirs).unwrap_or_default()
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
    fn needs_cgroup_slice() {
        // The node exits when apptainer cannot put a job into a delegated
        // slice, so an executor without one would only take the node down.
        let mut probe = seen(false, true);
        probe.apptainer.cgroup_ok = false;
        probe.apptainer.cgroup_root = None;
        assert!(resolve(ComputeBackend::Auto, &probe).is_none());
        assert!(resolve(ComputeBackend::Apptainer, &probe).is_none());
        assert_eq!(plan(ComputeBackend::Apptainer, &probe).note, Some(NO_SLICE));
        assert_eq!(plan(ComputeBackend::Auto, &probe).note, Some(NO_BACKEND));
        assert!(plan(ComputeBackend::Docker, &probe).note.is_none());
    }

    #[test]
    fn honours_explicit_choice() {
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
    fn lists_docker_sockets() {
        let paths = socket_paths(
            Some(Path::new("/run/user/1000")),
            Some(Path::new("/Users/me")),
        );
        assert_eq!(paths[0], Path::new("/run/user/1000/docker.sock"));
        assert!(paths.contains(&PathBuf::from("/Users/me/.colima/default/docker.sock")));
        assert!(paths.contains(&PathBuf::from("/Users/me/.orbstack/run/docker.sock")));
        assert_eq!(
            socket_paths(None, None),
            [PathBuf::from("/var/run/docker.sock")]
        );
    }

    #[test]
    fn keeps_existing_path() {
        // Directories already on the path stay where they are, once.
        let kept = extend_path(Some(OsString::from("/a:/usr/local/bin")), None);
        let dirs: Vec<PathBuf> = std::env::split_paths(&kept).collect();
        assert_eq!(dirs[0], Path::new("/a"));
        assert_eq!(
            dirs.iter()
                .filter(|dir| dir.as_path() == Path::new("/usr/local/bin"))
                .count(),
            1
        );
    }

    #[test]
    fn finds_user_slice() {
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
