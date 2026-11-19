use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_bytes: u64,
    pub max_files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArchiveError {
    Corrupt,
    UnsafePath,
    TooLarge,
    TooManyFiles,
    Io(String),
}

// Le contenu d'un dépôt est non fiable : seuls fichiers et dossiers réguliers sont écrits, sous
// `dest`, sans `..`, sans chemin absolu, sans lien (un lien pourrait pointer hors du dossier), et
// dans des limites de taille et de nombre (une archive peut gonfler à la décompression).
pub fn extract(tar_gz: &[u8], dest: &Path, limits: &Limits) -> Result<(), ArchiveError> {
    let io_error = |e: io::Error| ArchiveError::Io(e.to_string());
    fs::create_dir_all(dest).map_err(io_error)?;
    let decoder = GzDecoder::new(tar_gz).take(stream_cap(limits));
    let mut archive = Archive::new(decoder);
    let mut files = 0usize;
    let mut total = 0u64;
    for entry in archive.entries().map_err(|_| ArchiveError::Corrupt)? {
        let mut entry = entry.map_err(|_| ArchiveError::Corrupt)?;
        let kind = entry.header().entry_type();
        if !matches!(
            kind,
            EntryType::Regular | EntryType::Continuous | EntryType::Directory
        ) {
            continue;
        }
        let path = entry
            .path()
            .map_err(|_| ArchiveError::Corrupt)?
            .into_owned();
        let Some(target) = safe_target(&path, dest)? else {
            continue;
        };
        if kind == EntryType::Directory {
            fs::create_dir_all(&target).map_err(io_error)?;
            continue;
        }
        files += 1;
        if files > limits.max_files {
            return Err(ArchiveError::TooManyFiles);
        }
        total = total.saturating_add(entry.size());
        if total > limits.max_bytes {
            return Err(ArchiveError::TooLarge);
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io_error)?;
        }
        let mut file = fs::File::create(&target).map_err(io_error)?;
        io::copy(&mut entry, &mut file).map_err(|_| ArchiveError::Corrupt)?;
    }
    if total > limits.max_bytes {
        return Err(ArchiveError::TooLarge);
    }
    Ok(())
}

// Le flux tar contient aussi les en-têtes et le remplissage (1 Ko par fichier au plus) : la marge
// les couvre, la limite exacte sur le contenu est vérifiée entrée par entrée.
fn stream_cap(limits: &Limits) -> u64 {
    let overhead = (limits.max_files as u64 + 8).saturating_mul(1024);
    limits.max_bytes.saturating_add(overhead)
}

// GitHub place tout sous un dossier racine `proprietaire-depot-commit/` : on l'enlève.
// `Ok(None)` : l'entrée est la racine elle-même.
fn safe_target(path: &Path, dest: &Path) -> Result<Option<PathBuf>, ArchiveError> {
    let mut parts = path.components();
    match parts.next() {
        Some(Component::Normal(_)) => {}
        Some(Component::CurDir) => {}
        _ => return Err(ArchiveError::UnsafePath),
    }
    let mut target = dest.to_path_buf();
    let mut any = false;
    for part in parts {
        match part {
            Component::Normal(name) => {
                target.push(name);
                any = true;
            }
            _ => return Err(ArchiveError::UnsafePath),
        }
    }
    Ok(any.then_some(target))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::write::GzEncoder;
    use flate2::Compression;

    use super::*;

    const LIMITS: Limits = Limits {
        max_bytes: 1000,
        max_files: 10,
    };

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("platform-build-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    // Écrit un en-tête de la main : l'API de `tar` refuse justement les chemins dangereux.
    fn raw_entry(path: &str, kind: u8, data: &[u8]) -> Vec<u8> {
        let mut header = [0u8; 512];
        header[..path.len()].copy_from_slice(path.as_bytes());
        header[100..107].copy_from_slice(b"0000644");
        let size = format!("{:011o}", data.len());
        header[124..135].copy_from_slice(size.as_bytes());
        header[148..156].copy_from_slice(b"        ");
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        let checksum = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(checksum.as_bytes());
        let mut out = header.to_vec();
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
        out
    }

    fn archive(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut raw = Vec::new();
        for e in entries {
            raw.extend_from_slice(e);
        }
        raw.extend_from_slice(&[0u8; 1024]);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        let _ = encoder.write_all(&raw);
        encoder.finish().unwrap_or_default()
    }

    fn read(dir: &Path, name: &str) -> Option<String> {
        fs::read_to_string(dir.join(name)).ok()
    }

    #[test]
    fn files_land_under_the_destination_without_the_github_root_folder() {
        let dir = scratch("ok");
        let bytes = archive(&[
            raw_entry("alice-bot-abc123/", b'5', b""),
            raw_entry("alice-bot-abc123/bot.toml", b'0', b"name"),
            raw_entry("alice-bot-abc123/src/", b'5', b""),
            raw_entry("alice-bot-abc123/src/index.js", b'0', b"code"),
        ]);
        assert_eq!(extract(&bytes, &dir, &LIMITS), Ok(()));
        assert_eq!(read(&dir, "bot.toml").as_deref(), Some("name"));
        assert_eq!(read(&dir, "src/index.js").as_deref(), Some("code"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parent_directories_and_absolute_paths_are_refused_and_nothing_escapes() {
        for bad in [
            "root/../../escape.txt",
            "root/a/../../../escape.txt",
            "/etc/passwd",
            "../escape.txt",
        ] {
            let dir = scratch("escape");
            let bytes = archive(&[raw_entry(bad, b'0', b"x")]);
            assert_eq!(
                extract(&bytes, &dir, &LIMITS),
                Err(ArchiveError::UnsafePath),
                "{bad}"
            );
            assert!(!dir.parent().is_some_and(|p| p.join("escape.txt").exists()));
            let _ = fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn links_and_special_files_are_skipped_not_followed() {
        let dir = scratch("links");
        let bytes = archive(&[
            raw_entry("root/link", b'2', b""),
            raw_entry("root/hard", b'1', b""),
            raw_entry("root/dev", b'3', b""),
            raw_entry("root/fifo", b'6', b""),
            raw_entry("root/real.txt", b'0', b"ok"),
        ]);
        assert_eq!(extract(&bytes, &dir, &LIMITS), Ok(()));
        assert_eq!(read(&dir, "real.txt").as_deref(), Some("ok"));
        for skipped in ["link", "hard", "dev", "fifo"] {
            assert!(!dir.join(skipped).exists(), "{skipped}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_total_size_and_the_file_count_are_limited() {
        let dir = scratch("limits");
        let big = archive(&[
            raw_entry("r/a", b'0', &[b'x'; 600]),
            raw_entry("r/b", b'0', &[b'x'; 600]),
        ]);
        assert_eq!(extract(&big, &dir, &LIMITS), Err(ArchiveError::TooLarge));
        let exact = archive(&[
            raw_entry("r/a", b'0', &[b'x'; 500]),
            raw_entry("r/b", b'0', &[b'x'; 500]),
        ]);
        assert_eq!(extract(&exact, &scratch("exact"), &LIMITS), Ok(()));
        let many: Vec<Vec<u8>> = (0..11)
            .map(|i| raw_entry(&format!("r/f{i}"), b'0', b"x"))
            .collect();
        assert_eq!(
            extract(&archive(&many), &scratch("many"), &LIMITS),
            Err(ArchiveError::TooManyFiles)
        );
        let ten: Vec<Vec<u8>> = (0..10)
            .map(|i| raw_entry(&format!("r/f{i}"), b'0', b"x"))
            .collect();
        assert_eq!(extract(&archive(&ten), &scratch("ten"), &LIMITS), Ok(()));
        for name in ["limits", "exact", "many", "ten"] {
            let _ = fs::remove_dir_all(scratch(name));
        }
    }

    #[test]
    fn a_decompression_bomb_is_stopped_before_it_fills_the_disk() {
        let dir = scratch("bomb");
        let bytes = archive(&[raw_entry("r/zeros", b'0', &vec![0u8; 5000])]);
        assert!(bytes.len() < 1000, "the archive itself is small");
        assert_eq!(extract(&bytes, &dir, &LIMITS), Err(ArchiveError::TooLarge));
        assert!(fs::metadata(dir.join("zeros")).map_or(true, |m| m.len() <= 1000));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stream_cap_leaves_room_for_headers_and_padding() {
        assert_eq!(stream_cap(&LIMITS), 1000 + (10 + 8) * 1024);
        let huge = Limits {
            max_bytes: u64::MAX,
            max_files: 5,
        };
        assert_eq!(stream_cap(&huge), u64::MAX);
    }

    #[test]
    fn current_directory_markers_in_paths_are_harmless() {
        let dir = scratch("dots");
        let bytes = archive(&[
            raw_entry("./root/a.txt", b'0', b"a"),
            raw_entry("root/./b.txt", b'0', b"b"),
        ]);
        assert_eq!(extract(&bytes, &dir, &LIMITS), Ok(()));
        assert_eq!(read(&dir, "root/a.txt").as_deref(), Some("a"));
        assert_eq!(read(&dir, "b.txt").as_deref(), Some("b"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_archive_is_corrupt() {
        let dir = scratch("truncated");
        let whole = archive(&[raw_entry("r/a", b'0', &[b'x'; 400])]);
        let cut = &whole[..whole.len() / 2];
        assert_eq!(extract(cut, &dir, &LIMITS), Err(ArchiveError::Corrupt));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn garbage_is_corrupt_not_a_panic() {
        let dir = scratch("garbage");
        assert_eq!(
            extract(b"not a gzip", &dir, &LIMITS),
            Err(ArchiveError::Corrupt)
        );
        assert_eq!(extract(&[], &dir, &LIMITS), Err(ArchiveError::Corrupt));
        let _ = fs::remove_dir_all(&dir);
    }
}
