use std::path::PathBuf;
use std::process::Command;

use crate::args::docker_run_args;
use crate::spec::{valid_image, valid_name, RunSpec, SpecError};

#[derive(Debug)]
pub enum RunError {
    Spec(SpecError),
    Io(std::io::Error),
    Failed { code: Option<i32>, stderr: String },
}

impl From<SpecError> for RunError {
    fn from(error: SpecError) -> Self {
        Self::Spec(error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Running,
    Restarting,
    Exited(i32),
    Other(String),
}

fn parse_state(text: &str) -> ContainerState {
    let mut parts = text.split_whitespace();
    match (parts.next(), parts.next()) {
        (Some("running"), _) => ContainerState::Running,
        (Some("restarting"), _) => ContainerState::Restarting,
        (Some("exited"), Some(code)) => code.parse().map_or_else(
            |_| ContainerState::Other("exited".to_string()),
            ContainerState::Exited,
        ),
        (Some(other), _) => ContainerState::Other(other.to_string()),
        (None, _) => ContainerState::Other(String::new()),
    }
}

// Seul un container absent donne `None` ; toute autre panne (démon injoignable...) reste une erreur.
fn interpret_state(result: Result<String, RunError>) -> Result<Option<ContainerState>, RunError> {
    match result {
        Ok(text) => Ok(Some(parse_state(&text))),
        Err(RunError::Failed { stderr, .. }) if stderr.to_lowercase().contains("no such") => {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn interpret_exists(result: Result<String, RunError>) -> Result<bool, RunError> {
    match result {
        Ok(_) => Ok(true),
        Err(RunError::Failed { stderr, .. }) if stderr.to_lowercase().contains("no such") => {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn check_name(name: &str) -> Result<(), RunError> {
    if valid_name(name) {
        Ok(())
    } else {
        Err(SpecError::InvalidName.into())
    }
}

pub struct Docker {
    pub(crate) binary: PathBuf,
}

impl Docker {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    pub fn run(&self, spec: &RunSpec) -> Result<String, RunError> {
        let args = docker_run_args(spec)?;
        let mut command = Command::new(&self.binary);
        command.args(args);
        for (name, value) in &spec.env {
            command.env(name, value);
        }
        Ok(self.output(command)?.trim().to_string())
    }

    pub fn wait(&self, name: &str) -> Result<i32, RunError> {
        let out = self.simple(&["wait", name])?;
        out.trim().parse().map_err(|_| RunError::Failed {
            code: None,
            stderr: format!("unexpected output from docker wait: {out}"),
        })
    }

    pub fn logs(&self, name: &str) -> Result<String, RunError> {
        self.simple(&["logs", name])
    }

    pub fn remove(&self, name: &str) -> Result<(), RunError> {
        self.simple(&["rm", "-f", name]).map(|_| ())
    }

    pub fn server_version(&self) -> Result<String, RunError> {
        let mut command = Command::new(&self.binary);
        command.args(["version", "--format", "{{.Server.Version}}"]);
        Ok(self.output(command)?.trim().to_string())
    }

    pub fn image_exists(&self, image: &str) -> Result<bool, RunError> {
        if !valid_image(image) {
            return Err(SpecError::InvalidImage.into());
        }
        let mut command = Command::new(&self.binary);
        command.args(["image", "inspect", "--format", "{{.Id}}", image]);
        interpret_exists(self.output(command))
    }

    pub fn logs_tail(&self, name: &str, lines: u32) -> Result<String, RunError> {
        check_name(name)?;
        let mut command = Command::new(&self.binary);
        command.args(["logs", "--tail", &lines.to_string(), name]);
        self.output(command)
    }

    pub fn state(&self, name: &str) -> Result<Option<ContainerState>, RunError> {
        check_name(name)?;
        let mut command = Command::new(&self.binary);
        command.args([
            "inspect",
            "--format",
            "{{.State.Status}} {{.State.ExitCode}}",
            name,
        ]);
        interpret_state(self.output(command))
    }

    fn simple(&self, args: &[&str]) -> Result<String, RunError> {
        if let Some(name) = args.last() {
            if !valid_name(name) {
                return Err(SpecError::InvalidName.into());
            }
        }
        let mut command = Command::new(&self.binary);
        command.args(args);
        self.output(command)
    }

    fn output(&self, mut command: Command) -> Result<String, RunError> {
        let output = command.output().map_err(RunError::Io)?;
        if output.status.success() {
            // `docker logs` rejoue stdout et stderr du container ; on garde les deux.
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            Ok(text)
        } else {
            Err(RunError::Failed {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Tier;

    fn spec(name: &str, command: &[&str]) -> RunSpec {
        RunSpec {
            name: name.to_string(),
            image: "alpine:3".to_string(),
            tier: Tier::Free,
            network: "none".to_string(),
            env: vec![("DISCORD_TOKEN".to_string(), "s3cret".to_string())],
            command: command.iter().map(|s| (*s).to_string()).collect(),
            volume: None,
        }
    }

    fn docker() -> Docker {
        let path = std::env::var("DOCKER_BIN").unwrap_or_else(|_| "docker".to_string());
        Docker::new(path)
    }

    fn run_to_completion(name: &str, script: &str) -> String {
        let d = docker();
        let _ = d.remove(name);
        let started = d.run(&spec(name, &["sh", "-c", script]));
        assert!(started.is_ok(), "run failed: {started:?}");
        let code = d.wait(name);
        let logs = d.logs(name);
        let _ = d.remove(name);
        assert!(matches!(code, Ok(0)), "exit code: {code:?}, logs: {logs:?}");
        logs.unwrap_or_default()
    }

    #[test]
    fn container_states_are_parsed_from_docker_inspect() {
        assert_eq!(parse_state("running 0"), ContainerState::Running);
        assert_eq!(parse_state("restarting 1\n"), ContainerState::Restarting);
        assert_eq!(parse_state("exited 0"), ContainerState::Exited(0));
        assert_eq!(parse_state("exited 137\n"), ContainerState::Exited(137));
        assert_eq!(
            parse_state("exited x"),
            ContainerState::Other("exited".to_string())
        );
        assert_eq!(
            parse_state("created 0"),
            ContainerState::Other("created".to_string())
        );
        assert_eq!(parse_state(""), ContainerState::Other(String::new()));
    }

    #[test]
    fn only_a_missing_container_means_none() {
        let failed = |stderr: &str| RunError::Failed {
            code: Some(1),
            stderr: stderr.to_string(),
        };
        assert!(matches!(
            interpret_state(Err(failed("error: no such object: x"))),
            Ok(None)
        ));
        assert!(matches!(
            interpret_state(Err(failed("Error: No such container: x"))),
            Ok(None)
        ));
        assert!(matches!(
            interpret_state(Err(failed("Cannot connect to the Docker daemon"))),
            Err(RunError::Failed { .. })
        ));
        assert!(matches!(
            interpret_state(Err(RunError::Io(std::io::Error::other("boom")))),
            Err(RunError::Io(_))
        ));
        assert!(matches!(
            interpret_state(Ok("running 0".to_string())),
            Ok(Some(ContainerState::Running))
        ));
    }

    #[test]
    fn only_a_missing_image_means_false() {
        let failed = |stderr: &str| RunError::Failed {
            code: Some(1),
            stderr: stderr.to_string(),
        };
        assert!(matches!(
            interpret_exists(Ok("sha256:abc".to_string())),
            Ok(true)
        ));
        assert!(matches!(
            interpret_exists(Err(failed("error: no such image: x"))),
            Ok(false)
        ));
        assert!(matches!(
            interpret_exists(Err(failed("Cannot connect to the Docker daemon"))),
            Err(RunError::Failed { .. })
        ));
        assert!(matches!(
            interpret_exists(Err(RunError::Io(std::io::Error::other("boom")))),
            Err(RunError::Io(_))
        ));
    }

    #[test]
    fn image_names_are_checked_before_inspecting() {
        let d = Docker::new("this-binary-does-not-exist");
        assert!(matches!(
            d.image_exists("--all"),
            Err(RunError::Spec(SpecError::InvalidImage))
        ));
        assert!(matches!(d.image_exists("ok:1"), Err(RunError::Io(_))));
        assert!(matches!(d.server_version(), Err(RunError::Io(_))));
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn the_engine_version_and_local_images_are_reported() {
        let d = docker();
        assert!(d
            .server_version()
            .is_ok_and(|v| !v.is_empty() && v.contains('.')));
        assert!(matches!(d.image_exists("alpine:3"), Ok(true)));
        assert!(matches!(
            d.image_exists("platform-no-such-image:none"),
            Ok(false)
        ));
    }

    #[test]
    fn names_are_checked_before_state_and_logs() {
        let d = Docker::new("this-binary-does-not-exist");
        assert!(matches!(
            d.state("--all"),
            Err(RunError::Spec(SpecError::InvalidName))
        ));
        assert!(matches!(
            d.logs_tail("-x", 5),
            Err(RunError::Spec(SpecError::InvalidName))
        ));
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn state_reports_missing_running_and_exited_containers() {
        let d = docker();
        let name = "platform-test-state";
        let _ = d.remove(name);
        assert_eq!(d.state(name).ok(), Some(None));
        assert!(d.run(&spec(name, &["sleep", "60"])).is_ok());
        assert_eq!(d.state(name).ok(), Some(Some(ContainerState::Running)));
        let _ = d.remove(name);
        assert!(d.run(&spec(name, &["sh", "-c", "echo done"])).is_ok());
        assert!(matches!(d.wait(name), Ok(0)));
        assert_eq!(d.state(name).ok(), Some(Some(ContainerState::Exited(0))));
        let _ = d.remove(name);
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn logs_tail_keeps_only_the_last_lines() {
        let d = docker();
        let name = "platform-test-tail";
        let _ = d.remove(name);
        assert!(d
            .run(&spec(name, &["sh", "-c", "echo one; echo two; echo three"]))
            .is_ok());
        assert!(matches!(d.wait(name), Ok(0)));
        let tail = d.logs_tail(name, 2).unwrap_or_default();
        let _ = d.remove(name);
        assert!(tail.contains("two") && tail.contains("three"), "{tail}");
        assert!(!tail.contains("one"), "{tail}");
    }

    #[test]
    fn an_invalid_spec_never_reaches_docker() {
        let d = Docker::new("this-binary-does-not-exist");
        let bad = RunSpec {
            name: "--privileged".to_string(),
            ..spec("x", &[])
        };
        assert!(matches!(
            d.run(&bad),
            Err(RunError::Spec(SpecError::InvalidName))
        ));
        assert!(matches!(
            d.logs("-x"),
            Err(RunError::Spec(SpecError::InvalidName))
        ));
    }

    #[test]
    fn a_missing_binary_is_an_io_error() {
        let d = Docker::new("this-binary-does-not-exist");
        assert!(matches!(d.run(&spec("x", &[])), Err(RunError::Io(_))));
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn the_root_filesystem_is_read_only() {
        let logs = run_to_completion(
            "platform-test-ro",
            "touch /x 2>/dev/null && echo WRITABLE || echo READONLY",
        );
        assert!(logs.contains("READONLY"), "{logs}");
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn tmp_is_writable_but_not_executable() {
        let logs = run_to_completion(
            "platform-test-tmp",
            "echo '#!/bin/sh' > /tmp/x && chmod +x /tmp/x && (/tmp/x 2>/dev/null && echo EXEC || echo NOEXEC)",
        );
        assert!(logs.contains("NOEXEC"), "{logs}");
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn all_capabilities_are_dropped() {
        let logs = run_to_completion("platform-test-caps", "grep CapEff /proc/self/status");
        assert!(logs.contains("0000000000000000"), "{logs}");
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn the_token_reaches_the_container_through_the_environment() {
        let logs = run_to_completion("platform-test-env", "echo TOKEN=$DISCORD_TOKEN");
        assert!(logs.contains("TOKEN=s3cret"), "{logs}");
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn there_is_no_network_when_none_is_requested() {
        let logs = run_to_completion(
            "platform-test-net",
            "wget -q -T 3 -O /dev/null http://1.1.1.1 2>/dev/null && echo ONLINE || echo OFFLINE",
        );
        assert!(logs.contains("OFFLINE"), "{logs}");
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn wait_returns_the_real_exit_code() {
        let d = docker();
        let name = "platform-test-exit";
        let _ = d.remove(name);
        assert!(d.run(&spec(name, &["sh", "-c", "exit 3"])).is_ok());
        let code = d.wait(name);
        let _ = d.remove(name);
        assert!(matches!(code, Ok(3)), "{code:?}");
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn remove_deletes_a_running_container() {
        let d = docker();
        let name = "platform-test-rm";
        let _ = d.remove(name);
        assert!(d.run(&spec(name, &["sleep", "60"])).is_ok());
        assert!(d.remove(name).is_ok());
        assert!(matches!(d.logs(name), Err(RunError::Failed { .. })));
    }
}
