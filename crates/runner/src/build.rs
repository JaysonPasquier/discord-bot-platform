use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use crate::exec::{Docker, RunError};
use crate::spec::{valid_image, SpecError};

// Un contexte de build préparé par la plateforme : un dossier, un Dockerfile généré, une étiquette.
#[derive(Debug, Clone)]
pub struct BuildSpec {
    pub context: PathBuf,
    pub dockerfile: PathBuf,
    pub tag: String,
    pub timeout: Duration,
}

// Le code du créateur tourne déjà pendant le build (installation de dépendances, scripts npm
// postinstall, build.rs) : mêmes limites que le container final, sauf le réseau, encore
// ouvert ici parce que l'installation des paquets en a besoin (le filtrage réseau
// du build reste à traiter, comme celui du runtime).
const BUILD_MEMORY_MB: u32 = 2048;
const BUILD_PIDS_LIMIT: u32 = 512;
const BUILD_CPUS: &str = "2";

pub fn docker_build_args(spec: &BuildSpec) -> Result<Vec<String>, SpecError> {
    if !valid_image(&spec.tag) {
        return Err(SpecError::InvalidImage);
    }
    let memory = format!("{BUILD_MEMORY_MB}m");
    Ok(vec![
        "build".to_string(),
        "--progress=plain".to_string(),
        "--label".to_string(),
        "platform.managed=1".to_string(),
        "--memory".to_string(),
        memory.clone(),
        "--memory-swap".to_string(),
        memory,
        "--cpus".to_string(),
        BUILD_CPUS.to_string(),
        "--pids-limit".to_string(),
        BUILD_PIDS_LIMIT.to_string(),
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
        "--file".to_string(),
        spec.dockerfile.display().to_string(),
        "--tag".to_string(),
        spec.tag.clone(),
        spec.context.display().to_string(),
    ])
}

// Lit un flux dans un fil à part ; le résultat arrive par un canal pour pouvoir ne pas l'attendre
// éternellement : un processus petit-enfant peut garder le flux ouvert après la mort de `docker`.
fn drain(mut stream: impl Read + Send + 'static) -> Receiver<String> {
    let (send, receive) = channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes);
        let _ = send.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receive
}

const DRAIN_WAIT: Duration = Duration::from_secs(2);

impl Docker {
    // Renvoie le journal du build (sortie standard et d'erreur). Un build trop long est tué.
    pub fn build(&self, spec: &BuildSpec) -> Result<String, RunError> {
        let args = docker_build_args(spec)?;
        let mut child = Command::new(&self.binary)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(RunError::Io)?;
        let out = child.stdout.take().map(drain);
        let err = child.stderr.take().map(drain);
        let deadline = Instant::now() + spec.timeout;
        let status = loop {
            match child.try_wait().map_err(RunError::Io)? {
                Some(status) => break Some(status),
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                None => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        let collect = |stream: Option<Receiver<String>>| {
            stream
                .and_then(|r| r.recv_timeout(DRAIN_WAIT).ok())
                .unwrap_or_default()
        };
        let log = format!("{}{}", collect(out), collect(err));
        match status {
            Some(status) if status.success() => Ok(log),
            Some(status) => Err(RunError::Failed {
                code: status.code(),
                stderr: log,
            }),
            None => Err(RunError::Failed {
                code: None,
                stderr: format!(
                    "{log}\nbuild stopped: it took longer than {} seconds",
                    spec.timeout.as_secs()
                ),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(tag: &str) -> BuildSpec {
        BuildSpec {
            context: PathBuf::from("ctx"),
            dockerfile: PathBuf::from("ctx/Dockerfile.platform"),
            tag: tag.to_string(),
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn the_build_command_uses_the_generated_dockerfile_and_never_the_repositorys() {
        let args = docker_build_args(&spec("platform-bot-1-free:abc")).unwrap_or_default();
        assert_eq!(args[0], "build");
        let at = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        assert_eq!(at("--file").as_deref(), Some("ctx/Dockerfile.platform"));
        assert_eq!(at("--tag").as_deref(), Some("platform-bot-1-free:abc"));
        assert_eq!(at("--label").as_deref(), Some("platform.managed=1"));
        assert_eq!(args.last().map(String::as_str), Some("ctx"));
        assert!(args.contains(&"--progress=plain".to_string()));
    }

    #[test]
    fn the_build_is_resource_limited_like_the_runtime_container() {
        let args = docker_build_args(&spec("platform-bot-1-free:abc")).unwrap_or_default();
        let at = |flag: &str| {
            args.iter()
                .position(|a| a == flag)
                .and_then(|i| args.get(i + 1))
                .cloned()
        };
        assert_eq!(at("--memory").as_deref(), Some("2048m"));
        assert_eq!(at("--memory-swap").as_deref(), Some("2048m"));
        assert_eq!(at("--cpus").as_deref(), Some("2"));
        assert_eq!(at("--pids-limit").as_deref(), Some("512"));
        assert!(args.contains(&"--security-opt".to_string()));
        assert!(args.contains(&"no-new-privileges".to_string()));
    }

    #[test]
    fn a_tag_that_could_be_a_flag_is_refused() {
        for bad in ["", "--rm", "a b", "a;b", "$(x)"] {
            assert!(
                matches!(docker_build_args(&spec(bad)), Err(SpecError::InvalidImage)),
                "{bad}"
            );
        }
    }

    #[test]
    fn a_missing_docker_binary_is_an_io_error() {
        let docker = Docker::new("definitely-not-a-docker-binary");
        assert!(matches!(
            docker.build(&spec("platform-bot-1-free:abc")),
            Err(RunError::Io(_))
        ));
    }

    #[test]
    fn an_invalid_tag_never_starts_a_process() {
        let docker = Docker::new("definitely-not-a-docker-binary");
        assert!(matches!(
            docker.build(&spec("--rm")),
            Err(RunError::Spec(SpecError::InvalidImage))
        ));
    }

    // Un faux `docker` : un petit script qui écrit sur les deux sorties, dort ou échoue.
    fn fake_docker(name: &str, body_unix: &str, body_windows: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "platform-fake-docker-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        if cfg!(windows) {
            let path = dir.join("docker.cmd");
            let _ = std::fs::write(&path, format!("@echo off\r\n{body_windows}\r\n"));
            path
        } else {
            let path = dir.join("docker.sh");
            let _ = std::fs::write(&path, format!("#!/bin/sh\n{body_unix}\n"));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
            }
            path
        }
    }

    #[test]
    fn a_successful_build_returns_both_output_streams() {
        let docker = Docker::new(fake_docker(
            "ok",
            "echo from-out\necho from-err >&2",
            "echo from-out\r\necho from-err 1>&2",
        ));
        let log = docker
            .build(&spec("platform-bot-1-free:abc"))
            .unwrap_or_default();
        assert!(
            log.contains("from-out") && log.contains("from-err"),
            "{log}"
        );
    }

    #[test]
    fn a_failing_build_reports_its_exit_code_and_its_log() {
        let docker = Docker::new(fake_docker(
            "fail",
            "echo step-failed\nexit 3",
            "echo step-failed\r\nexit /b 3",
        ));
        match docker.build(&spec("platform-bot-1-free:abc")) {
            Err(RunError::Failed { code, stderr }) => {
                assert_eq!(code, Some(3));
                assert!(stderr.contains("step-failed"), "{stderr}");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_build_that_takes_too_long_is_stopped() {
        let docker = Docker::new(fake_docker("slow", "sleep 30", "ping -n 30 127.0.0.1 >nul"));
        let mut s = spec("platform-bot-1-free:abc");
        s.timeout = Duration::from_millis(600);
        let started = Instant::now();
        let result = docker.build(&s);
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the build was not stopped"
        );
        match result {
            Err(RunError::Failed { code: None, stderr }) => {
                assert!(stderr.contains("took longer than 0 seconds"), "{stderr}")
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
    }
}
