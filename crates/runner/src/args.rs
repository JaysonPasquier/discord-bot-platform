use crate::spec::{
    valid_env_name, valid_image, valid_name, RunSpec, SpecError, MAX_ENV, VOLUME_MOUNT_PATH,
};

// Les valeurs d'environnement (tokens) ne figurent jamais dans les arguments : `-e NOM`
// fait lire la valeur à docker dans son propre environnement, invisible dans `ps`.
pub fn docker_run_args(spec: &RunSpec) -> Result<Vec<String>, SpecError> {
    if !valid_name(&spec.name) {
        return Err(SpecError::InvalidName);
    }
    if !valid_image(&spec.image) {
        return Err(SpecError::InvalidImage);
    }
    if !valid_name(&spec.network) {
        return Err(SpecError::InvalidNetwork);
    }
    if let Some(volume) = &spec.volume {
        if !valid_name(volume) {
            return Err(SpecError::InvalidVolume);
        }
    }
    if spec.env.len() > MAX_ENV {
        return Err(SpecError::TooManyEnv);
    }
    for (name, value) in &spec.env {
        if !valid_env_name(name) {
            return Err(SpecError::InvalidEnvName(name.clone()));
        }
        if value.contains('\0') {
            return Err(SpecError::InvalidEnvValue(name.clone()));
        }
    }
    if spec.command.iter().any(|a| a.contains('\0')) {
        return Err(SpecError::InvalidCommand);
    }

    let memory = format!("{}m", spec.tier.memory_mb());
    let mut args: Vec<String> = [
        "run",
        "-d",
        "--name",
        &spec.name,
        "--memory",
        &memory,
        "--memory-swap",
        &memory,
        "--cpus",
        spec.tier.cpus(),
        "--pids-limit",
        "128",
        "--read-only",
        "--tmpfs",
        "/tmp:rw,noexec,nosuid,size=64m",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "65534:65534",
        "--restart",
        "on-failure:10",
        "--network",
        &spec.network,
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    if let Some(volume) = &spec.volume {
        args.push("-v".to_string());
        args.push(format!("{volume}:{VOLUME_MOUNT_PATH}"));
    }
    for (name, _) in &spec.env {
        args.push("-e".to_string());
        args.push(name.clone());
    }
    args.push(spec.image.clone());
    args.extend(spec.command.iter().cloned());
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::Tier;

    fn spec() -> RunSpec {
        RunSpec {
            name: "bot-1-free".to_string(),
            image: "bot:1".to_string(),
            tier: Tier::Free,
            network: "platform-bots".to_string(),
            env: vec![("DISCORD_TOKEN".to_string(), "s3cret".to_string())],
            command: vec![],
            volume: Some("bot-1-data".to_string()),
        }
    }

    fn args(spec: &RunSpec) -> Vec<String> {
        match docker_run_args(spec) {
            Ok(a) => a,
            Err(e) => panic!("unexpected error: {e:?}"),
        }
    }

    fn value_after(args: &[String], flag: &str) -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    }

    #[test]
    fn the_command_starts_with_a_detached_named_run() {
        let a = args(&spec());
        assert_eq!(&a[..2], ["run", "-d"]);
        assert_eq!(value_after(&a, "--name").as_deref(), Some("bot-1-free"));
    }

    #[test]
    fn free_tier_gets_the_free_quotas_with_swap_disabled() {
        let a = args(&spec());
        assert_eq!(value_after(&a, "--memory").as_deref(), Some("128m"));
        assert_eq!(value_after(&a, "--memory-swap").as_deref(), Some("128m"));
        assert_eq!(value_after(&a, "--cpus").as_deref(), Some("0.1"));
    }

    #[test]
    fn premium_tier_gets_the_premium_quotas() {
        let a = args(&RunSpec {
            tier: Tier::Premium,
            ..spec()
        });
        assert_eq!(value_after(&a, "--memory").as_deref(), Some("512m"));
        assert_eq!(value_after(&a, "--memory-swap").as_deref(), Some("512m"));
        assert_eq!(value_after(&a, "--cpus").as_deref(), Some("1"));
    }

    #[test]
    fn the_container_is_hardened() {
        let a = args(&spec());
        assert!(a.contains(&"--read-only".to_string()));
        assert_eq!(value_after(&a, "--cap-drop").as_deref(), Some("ALL"));
        assert_eq!(
            value_after(&a, "--security-opt").as_deref(),
            Some("no-new-privileges")
        );
        assert_eq!(value_after(&a, "--user").as_deref(), Some("65534:65534"));
        assert_eq!(value_after(&a, "--pids-limit").as_deref(), Some("128"));
        assert_eq!(
            value_after(&a, "--tmpfs").as_deref(),
            Some("/tmp:rw,noexec,nosuid,size=64m")
        );
    }

    #[test]
    fn restarts_are_capped_at_ten_failures() {
        assert_eq!(
            value_after(&args(&spec()), "--restart").as_deref(),
            Some("on-failure:10")
        );
    }

    #[test]
    fn the_volume_is_mounted_when_given_and_left_out_otherwise() {
        let a = args(&spec());
        assert_eq!(value_after(&a, "-v").as_deref(), Some("bot-1-data:/data"));
        let none = args(&RunSpec {
            volume: None,
            ..spec()
        });
        assert!(!none.contains(&"-v".to_string()));
    }

    #[test]
    fn an_option_looking_volume_name_is_rejected() {
        let bad = RunSpec {
            volume: Some("--privileged".to_string()),
            ..spec()
        };
        assert_eq!(docker_run_args(&bad), Err(SpecError::InvalidVolume));
    }

    #[test]
    fn the_network_is_the_requested_one() {
        assert_eq!(
            value_after(&args(&spec()), "--network").as_deref(),
            Some("platform-bots")
        );
    }

    #[test]
    fn secret_values_never_appear_in_the_arguments() {
        let a = args(&spec());
        assert!(a.iter().all(|x| !x.contains("s3cret")));
        let i = a.iter().position(|x| x == "-e");
        assert_eq!(
            i.and_then(|i| a.get(i + 1)).map(String::as_str),
            Some("DISCORD_TOKEN")
        );
    }

    #[test]
    fn the_image_comes_last_then_the_command() {
        let a = args(&RunSpec {
            command: vec!["sh".to_string(), "-c".to_string(), "echo hi".to_string()],
            ..spec()
        });
        assert_eq!(&a[a.len() - 4..], ["bot:1", "sh", "-c", "echo hi"]);
        assert_eq!(args(&spec()).last().map(String::as_str), Some("bot:1"));
    }

    #[test]
    fn every_env_variable_gets_its_own_flag() {
        let a = args(&RunSpec {
            env: vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "2".to_string()),
            ],
            ..spec()
        });
        assert_eq!(a.iter().filter(|x| *x == "-e").count(), 2);
    }

    #[test]
    fn option_looking_names_images_and_networks_are_rejected() {
        let bad_name = RunSpec {
            name: "--privileged".to_string(),
            ..spec()
        };
        assert_eq!(docker_run_args(&bad_name), Err(SpecError::InvalidName));
        let bad_image = RunSpec {
            image: "--privileged".to_string(),
            ..spec()
        };
        assert_eq!(docker_run_args(&bad_image), Err(SpecError::InvalidImage));
        let bad_network = RunSpec {
            network: "host --privileged".to_string(),
            ..spec()
        };
        assert_eq!(
            docker_run_args(&bad_network),
            Err(SpecError::InvalidNetwork)
        );
    }

    #[test]
    fn env_names_and_values_are_checked() {
        let bad_name = RunSpec {
            env: vec![("a=b".to_string(), "x".to_string())],
            ..spec()
        };
        assert_eq!(
            docker_run_args(&bad_name),
            Err(SpecError::InvalidEnvName("a=b".to_string()))
        );
        let bad_value = RunSpec {
            env: vec![("A".to_string(), "x\0y".to_string())],
            ..spec()
        };
        assert_eq!(
            docker_run_args(&bad_value),
            Err(SpecError::InvalidEnvValue("A".to_string()))
        );
    }

    #[test]
    fn at_most_32_env_variables() {
        let many = |n: usize| -> Vec<(String, String)> {
            (0..n).map(|i| (format!("V{i}"), "x".to_string())).collect()
        };
        assert!(docker_run_args(&RunSpec {
            env: many(32),
            ..spec()
        })
        .is_ok());
        assert_eq!(
            docker_run_args(&RunSpec {
                env: many(33),
                ..spec()
            }),
            Err(SpecError::TooManyEnv)
        );
    }

    #[test]
    fn a_nul_byte_in_the_command_is_rejected() {
        let bad = RunSpec {
            command: vec!["a\0b".to_string()],
            ..spec()
        };
        assert_eq!(docker_run_args(&bad), Err(SpecError::InvalidCommand));
    }
}
