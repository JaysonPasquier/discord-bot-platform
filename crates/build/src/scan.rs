// Scan statique du code source d'un bot avant compilation.
// Détecte les patterns dangereux connus dans les fichiers JS/TS/JSON.
// Ce n'est pas un substitut à l'isolation Docker, mais une couche de défense précoce.
use std::fs;
use std::path::Path;

// (pattern, description)
const FORBIDDEN: &[(&str, &str)] = &[
    ("child_process", "shell execution via child_process"),
    ("/proc/self", "proc filesystem self-access"),
    ("/proc/net", "proc network information access"),
    ("/etc/passwd", "system password file access"),
    ("/etc/shadow", "system shadow file access"),
    ("/.ssh/", "SSH key directory access"),
    ("__import__", "dynamic Python import"),
    ("eval(", "eval() call"),
    ("new Function(", "dynamic Function constructor"),
    ("execSync(", "synchronous shell execution"),
    ("spawnSync(", "synchronous process spawn"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanViolation {
    pub file: String,
    pub line: usize,
    pub pattern: &'static str,
    pub description: &'static str,
}

pub fn scan_dir(dir: &Path) -> Vec<ScanViolation> {
    let mut violations = Vec::new();
    scan_recursive(dir, dir, &mut violations);
    violations
}

fn scan_recursive(root: &Path, dir: &Path, out: &mut Vec<ScanViolation>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Ok(ft) = entry.file_type() {
            if ft.is_dir() {
                // Ignore node_modules et dossiers cachés.
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if s == "node_modules" || s.starts_with('.') {
                    continue;
                }
                scan_recursive(root, &path, out);
            } else if ft.is_file() && should_scan(&path) {
                scan_file(root, &path, out);
            }
        }
    }
}

fn should_scan(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("js" | "ts" | "mjs" | "cjs" | "jsx" | "tsx")
    )
}

fn scan_file(root: &Path, path: &Path, out: &mut Vec<ScanViolation>) {
    let Ok(content) = fs::read_to_string(path) else {
        return;
    };
    let rel = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().to_string());
    for (lineno, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        // Ignorer les commentaires de ligne.
        if trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }
        for &(pattern, description) in FORBIDDEN {
            if line.contains(pattern) {
                out.push(ScanViolation {
                    file: rel.clone(),
                    line: lineno + 1,
                    pattern,
                    description,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn temp_dir() -> TempDir {
        match TempDir::new() {
            Ok(d) => d,
            Err(e) => panic!("cannot create temp dir: {e}"),
        }
    }

    fn write(dir: &Path, name: &str, content: &str) {
        let mut f = match fs::File::create(dir.join(name)) {
            Ok(f) => f,
            Err(e) => panic!("cannot create {name}: {e}"),
        };
        if let Err(e) = write!(f, "{content}") {
            panic!("cannot write {name}: {e}");
        }
    }

    #[test]
    fn clean_file_has_no_violations() {
        let d = temp_dir();
        write(d.path(), "index.ts", "console.log('hello world');\n");
        assert!(scan_dir(d.path()).is_empty());
    }

    #[test]
    fn child_process_import_is_flagged() {
        let d = temp_dir();
        write(d.path(), "bot.js", "const cp = require('child_process');\n");
        let v = scan_dir(d.path());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].pattern, "child_process");
        assert_eq!(v[0].line, 1);
    }

    #[test]
    fn line_comment_is_not_flagged() {
        let d = temp_dir();
        write(
            d.path(),
            "a.js",
            "// require('child_process')\nconsole.log(1);\n",
        );
        assert!(scan_dir(d.path()).is_empty());
    }

    #[test]
    fn proc_access_is_flagged() {
        let d = temp_dir();
        write(
            d.path(),
            "a.ts",
            "const f = fs.readFileSync('/proc/self/mem');\n",
        );
        let v = scan_dir(d.path());
        assert!(!v.is_empty());
    }

    #[test]
    fn node_modules_are_not_scanned() {
        let d = temp_dir();
        if let Err(e) = fs::create_dir(d.path().join("node_modules")) {
            panic!("cannot create node_modules: {e}");
        }
        write(
            &d.path().join("node_modules"),
            "evil.js",
            "require('child_process').exec('rm -rf /');\n",
        );
        assert!(scan_dir(d.path()).is_empty());
    }

    #[test]
    fn multiple_violations_are_all_reported() {
        let d = temp_dir();
        write(
            d.path(),
            "bad.js",
            "eval('x');\nconst cp = require('child_process');\n",
        );
        let v = scan_dir(d.path());
        assert_eq!(v.len(), 2);
    }
}
