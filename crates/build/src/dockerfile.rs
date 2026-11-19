use platform_config::Language;

// La plateforme écrit elle-même le Dockerfile : celui du dépôt n'est jamais utilisé, et le créateur
// ne choisit ni l'image de base ni les étapes de build. Seule sa commande de démarrage
// est reprise, échappée comme une chaîne JSON : aucun saut de ligne ne peut ajouter une instruction.
pub fn dockerfile(language: Language, start_command: &str, with_sdk: bool) -> String {
    let command = serde_json_string(start_command);
    let cmd = format!(r#"CMD ["sh", "-c", {command}]"#);
    match language {
        Language::Node => {
            let sdk = if with_sdk {
                "COPY .platform-sdk /app/node_modules/platform-sdk\n"
            } else {
                ""
            };
            format!(
                "FROM node:24-alpine
WORKDIR /app
COPY . .
RUN if [ -f package-lock.json ]; then npm ci --omit=dev --ignore-scripts --no-audit --no-fund; elif [ -f package.json ]; then npm install --omit=dev --ignore-scripts --no-audit --no-fund; fi
{sdk}USER node
{cmd}
"
            )
        }
        Language::Python => format!(
            "FROM python:3.13-slim
WORKDIR /app
RUN useradd --create-home app
COPY . .
RUN if [ -f requirements.txt ]; then pip install --no-cache-dir -r requirements.txt; fi
USER app
{cmd}
"
        ),
        Language::Rust => format!(
            "FROM rust:1-slim\nWORKDIR /app\nRUN useradd --create-home app\nCOPY . .\n\
RUN cargo build --release && chown -R app /app\nUSER app\n{cmd}\n"
        ),
    }
}

// Échappement JSON minimal, sans dépendance : guillemets, antislash et caractères de contrôle.
fn serde_json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_language_gets_its_own_base_image_and_a_non_root_user() {
        let node = dockerfile(Language::Node, "node index.js", false);
        assert!(node.starts_with("FROM node:24-alpine\n") && node.contains("USER node\n"));
        assert!(node.contains("--ignore-scripts"));
        let python = dockerfile(Language::Python, "python main.py", false);
        assert!(python.starts_with("FROM python:3.13-slim\n") && python.contains("USER app\n"));
        let rust = dockerfile(Language::Rust, "./target/release/bot", false);
        assert!(rust.starts_with("FROM rust:1-slim\n") && rust.contains("cargo build --release"));
        assert!(rust.contains("USER app\n"));
    }

    #[test]
    fn the_start_command_is_a_single_json_string_in_the_cmd_line() {
        let d = dockerfile(Language::Node, "node index.js --flag", false);
        assert!(
            d.ends_with("CMD [\"sh\", \"-c\", \"node index.js --flag\"]\n"),
            "{d}"
        );
    }

    #[test]
    fn quotes_backslashes_and_control_characters_cannot_add_instructions() {
        let d = dockerfile(Language::Node, "a\"b\\c\nRUN evil\u{7}", false);
        let last = d.lines().last().unwrap_or_default();
        assert_eq!(last, r#"CMD ["sh", "-c", "a\"b\\c\u000aRUN evil\u0007"]"#);
        assert_eq!(d.lines().filter(|l| l.starts_with("RUN evil")).count(), 0);
        assert_eq!(d.lines().filter(|l| l.starts_with("CMD")).count(), 1);
    }

    #[test]
    fn the_sdk_is_copied_only_for_node_and_only_when_present() {
        let with = dockerfile(Language::Node, "x", true);
        assert!(with.contains("COPY .platform-sdk /app/node_modules/platform-sdk\n"));
        assert!(!dockerfile(Language::Node, "x", false).contains(".platform-sdk"));
        assert!(!dockerfile(Language::Python, "x", true).contains(".platform-sdk"));
        assert!(!dockerfile(Language::Rust, "x", true).contains(".platform-sdk"));
        let copy = with.find(".platform-sdk").unwrap_or(0);
        let install = with.find("npm ci").unwrap_or(usize::MAX);
        assert!(
            install < copy,
            "the SDK must come after the install, which wipes node_modules"
        );
    }
}
