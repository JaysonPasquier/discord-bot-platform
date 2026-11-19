use platform_runner::{ContainerState, Docker, RunSpec};

// Les erreurs Docker (chemins, messages bruts) restent côté serveur : la page n'affiche qu'un texte fixe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerError;

pub trait Containers: Send + Sync + 'static {
    fn deploy(&self, spec: &RunSpec) -> Result<(), ContainerError>;
    fn stop(&self, name: &str) -> Result<(), ContainerError>;
    fn state(&self, name: &str) -> Result<Option<ContainerState>, ContainerError>;
    fn logs(&self, name: &str, lines: u32) -> Result<String, ContainerError>;
    fn image_exists(&self, image: &str) -> Result<bool, ContainerError>;
}

pub struct DockerContainers(pub Docker);

impl Containers for DockerContainers {
    fn deploy(&self, spec: &RunSpec) -> Result<(), ContainerError> {
        let _ = self.0.remove(&spec.name);
        self.0.run(spec).map(|_| ()).map_err(|_| ContainerError)
    }

    fn stop(&self, name: &str) -> Result<(), ContainerError> {
        self.0.remove(name).map_err(|_| ContainerError)
    }

    fn state(&self, name: &str) -> Result<Option<ContainerState>, ContainerError> {
        self.0.state(name).map_err(|_| ContainerError)
    }

    fn logs(&self, name: &str, lines: u32) -> Result<String, ContainerError> {
        self.0.logs_tail(name, lines).map_err(|_| ContainerError)
    }

    fn image_exists(&self, image: &str) -> Result<bool, ContainerError> {
        self.0.image_exists(image).map_err(|_| ContainerError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_runner::Tier;

    fn docker() -> DockerContainers {
        let binary = std::env::var("DOCKER_BIN").unwrap_or_else(|_| "docker".to_string());
        DockerContainers(Docker::new(binary))
    }

    fn spec(name: &str, script: &str) -> RunSpec {
        RunSpec {
            name: name.to_string(),
            image: "alpine:3".to_string(),
            tier: Tier::Free,
            network: "none".to_string(),
            env: vec![],
            command: vec!["sh".to_string(), "-c".to_string(), script.to_string()],
            volume: None,
        }
    }

    #[test]
    fn invalid_container_names_are_errors_not_commands() {
        let c = docker();
        assert_eq!(c.state("--all"), Err(ContainerError));
        assert_eq!(c.logs("--all", 5), Err(ContainerError));
        assert_eq!(c.stop("--all"), Err(ContainerError));
    }

    #[test]
    #[ignore = "needs a running Docker engine"]
    fn a_container_is_deployed_inspected_replaced_and_stopped() {
        let c = docker();
        let name = "platform-web-test-lifecycle";
        let _ = c.stop(name);
        assert_eq!(c.state(name), Ok(None));
        assert!(c
            .deploy(&spec(name, "echo hello-from-bot; sleep 30"))
            .is_ok());
        assert_eq!(c.state(name), Ok(Some(ContainerState::Running)));
        let mut logs = String::new();
        for _ in 0..20 {
            logs = c.logs(name, 10).unwrap_or_default();
            if logs.contains("hello-from-bot") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        assert!(logs.contains("hello-from-bot"), "{logs}");
        assert!(c.deploy(&spec(name, "echo replaced; sleep 30")).is_ok());
        let mut replaced = String::new();
        for _ in 0..20 {
            replaced = c.logs(name, 10).unwrap_or_default();
            if replaced.contains("replaced") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        assert!(
            replaced.contains("replaced") && !replaced.contains("hello-from-bot"),
            "{replaced}"
        );
        assert!(c.stop(name).is_ok());
        assert_eq!(c.state(name), Ok(None));
    }
}
