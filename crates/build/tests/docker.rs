// Nécessite un moteur Docker : `cargo test -p platform-build -- --ignored`.
use std::sync::Arc;
use std::time::Duration;

use platform_build::{image_tag, BuildRequest, DockerBuilder, ImageBuilder};
use platform_github::{Branch, GitHub, GitHubError, Installation, Repo};
use platform_runner::Docker;

const SHA: &str = "abcdef0123456789abcdef0123456789abcdef01";

struct OneRepo(Vec<u8>);

impl GitHub for OneRepo {
    fn authorize_url(&self, _: &str, _: &str) -> Option<String> {
        None
    }
    fn install_url(&self) -> Option<String> {
        None
    }
    fn user_installations(&self, _: &str, _: &str) -> Result<Vec<Installation>, GitHubError> {
        Err(GitHubError::NotConfigured)
    }
    fn repositories(&self, _: u64) -> Result<Vec<Repo>, GitHubError> {
        Ok(vec![])
    }
    fn branches(&self, _: u64, _: &str) -> Result<Vec<Branch>, GitHubError> {
        Ok(vec![])
    }
    fn branch_head(&self, _: u64, _: &str, _: &str) -> Result<String, GitHubError> {
        Ok(SHA.to_string())
    }
    fn download(&self, _: u64, _: &str, _: &str) -> Result<Vec<u8>, GitHubError> {
        Ok(self.0.clone())
    }
}

fn repo(files: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        let _ = builder.append_data(
            &mut header,
            format!("alice-bot-abc/{path}"),
            content.as_bytes(),
        );
    }
    let raw = builder.into_inner().unwrap_or_default();
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = std::io::Write::write_all(&mut encoder, &raw);
    encoder.finish().unwrap_or_default()
}

fn builder(files: &[(&str, &str)], sdk: Option<std::path::PathBuf>) -> (DockerBuilder, Docker) {
    let docker =
        || Docker::new(std::env::var("DOCKER_BIN").unwrap_or_else(|_| "docker".to_string()));
    let work = std::env::temp_dir().join(format!("platform-build-docker-{}", std::process::id()));
    (
        DockerBuilder::new(
            Arc::new(OneRepo(repo(files))),
            docker(),
            work,
            sdk,
            Duration::from_secs(300),
        ),
        docker(),
    )
}

const CONFIG: &str = r#"
schema_version = 1
name = "Alice Bot"
language = "node"
support_contact = "alice@example.com"
[free]
start_command = "node index.js"
"#;

fn request(product: u64) -> BuildRequest {
    BuildRequest {
        installation: 1,
        repo: "alice/bot".to_string(),
        sha: SHA.to_string(),
        product,
        tier: "free".to_string(),
    }
}

#[test]
#[ignore = "needs a running Docker engine"]
fn a_repository_is_built_into_an_image_that_runs_its_start_command() {
    let sdk = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/node"));
    let (b, docker) = builder(
        &[
            ("bot.toml", CONFIG),
            ("package.json", r#"{"name":"alice-bot","version":"1.0.0","private":true,"type":"module"}"#),
            ("index.js", "import { PlatformClient } from 'platform-sdk';\nconsole.log('hello-from-built-bot', typeof PlatformClient);\n"),
            ("Dockerfile", "FROM scratch\nRUN exit 1\n"),
        ],
        Some(sdk),
    );
    let mut steps = Vec::new();
    let out = b.build(&request(990_001), &mut |s| steps.push(s.to_string()));
    let Ok(out) = out else {
        panic!("build failed: {:?}", out.err().map(|e| e.to_string()));
    };
    assert_eq!(out.image, image_tag(990_001, "free", SHA));
    assert_eq!(docker.image_exists(&out.image).ok(), Some(true));
    assert_eq!(steps.len(), 3);
    let run = std::process::Command::new("docker")
        .args(["run", "--rm", &out.image])
        .output();
    let stdout = run
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    assert!(stdout.contains("hello-from-built-bot function"), "{stdout}");
    let _ = std::process::Command::new("docker")
        .args(["rmi", "-f", &out.image])
        .output();
}

#[test]
#[ignore = "needs a running Docker engine"]
fn a_failing_install_returns_the_docker_log_and_leaves_no_image() {
    let (b, docker) = builder(
        &[
            ("bot.toml", CONFIG),
            (
                "package.json",
                r#"{"name":"x","version":"1.0.0","dependencies":{"definitely-not-a-real-package-zzz":"1.0.0"}}"#,
            ),
        ],
        None,
    );
    let result = b.build(&request(990_002), &mut |_| {});
    let Err(error) = result else {
        panic!("the build should fail");
    };
    assert!(
        error.to_string().starts_with("docker build failed"),
        "{error}"
    );
    assert_eq!(
        docker.image_exists(&image_tag(990_002, "free", SHA)).ok(),
        Some(false)
    );
}

fn template_files() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/github-bot");
    let mut files = Vec::new();
    for name in [
        "bot.toml",
        "package.json",
        "package-lock.json",
        "index.js",
        "src/commands.js",
    ] {
        let text = std::fs::read_to_string(root.join(name)).unwrap_or_default();
        assert!(!text.is_empty(), "{name} is missing from the template");
        files.push((name.to_string(), text));
    }
    files
}

#[test]
#[ignore = "needs a running Docker engine and the npm registry"]
fn the_example_repository_template_builds_and_can_load_the_sdk() {
    let files = template_files();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(n, t)| (n.as_str(), t.as_str()))
        .collect();
    let sdk = std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../sdk/node"));
    for tier in ["free", "premium"] {
        let (b, _) = builder(&refs, Some(sdk.clone()));
        let mut request = request(990_010);
        request.tier = tier.to_string();
        let out = b.build(&request, &mut |_| {});
        let Ok(out) = out else {
            panic!(
                "build failed for {tier}: {:?}",
                out.err().map(|e| e.to_string())
            );
        };
        let check = std::process::Command::new("docker")
            .args(["run", "--rm", &out.image, "node", "--check", "index.js"])
            .output();
        assert!(
            check.is_ok_and(|o| o.status.success()),
            "index.js does not parse"
        );
        let load = std::process::Command::new("docker")
            .args([
                "run",
                "--rm",
                &out.image,
                "node",
                "-e",
                "import('platform-sdk').then(m => console.log(typeof m.PlatformClient))",
            ])
            .output();
        let stdout = load
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        assert!(stdout.contains("function"), "{stdout}");
        let _ = std::process::Command::new("docker")
            .args(["rmi", "-f", &out.image])
            .output();
    }
}
