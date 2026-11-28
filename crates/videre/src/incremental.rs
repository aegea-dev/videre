//! Shared change-detection for the walk: given the stored row signatures, decide
//! whether a walked file still needs processing. Used by `scan` and by `watch`'s
//! scan stage, so the "skip unchanged files" rule has one definition.

use std::collections::HashMap;
use std::path::Path;
use videre_core::db::{is_current, mtime_iso, RowSig};
use videre_core::ignored_files::IgnoredFile;

pub struct Candidates {
    pub paths: Vec<std::path::PathBuf>,
    pub ignored: usize,
}

/// Both scan and event indexing skip unchanged media and unchanged content
/// exclusions. A forced scan bypasses both sets of signatures.
pub fn candidates(
    conn: &rusqlite::Connection,
    paths: Vec<std::path::PathBuf>,
    force: bool,
) -> rusqlite::Result<Candidates> {
    if force {
        return Ok(Candidates { paths, ignored: 0 });
    }
    let sigs = videre_core::db::stored_signatures(conn).unwrap_or_default();
    let ignored = videre_core::ignored_files::stored_signatures(conn)?;
    let mut result = Candidates {
        paths: Vec::new(),
        ignored: 0,
    };
    for path in paths {
        match file_state(&sigs, &ignored, &path) {
            FileState::Needed => result.paths.push(path),
            FileState::Ignored => result.ignored += 1,
            FileState::Current => {}
        }
    }
    Ok(result)
}

enum FileState {
    Needed,
    Current,
    Ignored,
}

fn file_state(
    sigs: &HashMap<String, RowSig>,
    ignored: &HashMap<String, IgnoredFile>,
    path: &Path,
) -> FileState {
    let key = path.to_string_lossy();
    let record = sigs.get(key.as_ref());
    let exclusion = ignored.get(key.as_ref());
    if record.is_none() && exclusion.is_none() {
        return FileState::Needed;
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return FileState::Needed;
    };
    let mtime = meta.modified().ok().map(mtime_iso);
    if exclusion.is_some_and(|sig| {
        videre_core::db::is_unchanged(
            sig.size_bytes,
            sig.modified_at.as_deref(),
            meta.len(),
            mtime.as_deref(),
        )
    }) {
        FileState::Ignored
    } else if record.is_some_and(|sig| is_current(sig, meta.len(), mtime.as_deref())) {
        FileState::Current
    } else {
        FileState::Needed
    }
}

/// Whether a walked path needs processing: a new file (no row), a changed file
/// (size or mtime differs), a file that cannot be stat'd (let the hash path deal
/// with it), or a row missing its mime. Returns false only when the row is
/// already current.
pub fn needs_processing(sigs: &HashMap<String, RowSig>, path: &Path) -> bool {
    matches!(file_state(sigs, &HashMap::new(), path), FileState::Needed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn sig_for(path: &Path, has_mime: bool) -> RowSig {
        let meta = fs::metadata(path).unwrap();
        RowSig {
            size_bytes: meta.len(),
            modified_at: meta.modified().ok().map(mtime_iso),
            has_mime,
        }
    }

    #[test]
    fn ignored_candidates_require_known_reason_and_matching_stamps() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("._çiçek.jpg");
        fs::write(&path, b"cached header").unwrap();
        let sig = sig_for(&path, true);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        videre_core::library_db::ensure_scan_schema(&conn).unwrap();
        for (name, reason, modified_at, force, skipped) in [
            (
                "current",
                "appledouble",
                sig.modified_at.clone(),
                false,
                true,
            ),
            (
                "unknown reason",
                "future-format",
                sig.modified_at.clone(),
                false,
                false,
            ),
            ("no stored mtime", "appledouble", None, false, false),
            (
                "forced",
                "appledouble",
                sig.modified_at.clone(),
                true,
                false,
            ),
        ] {
            videre_core::ignored_files::write(
                &conn,
                &[IgnoredFile {
                    path: path.to_string_lossy().into_owned(),
                    size_bytes: sig.size_bytes,
                    modified_at,
                    reason: reason.to_owned(),
                }],
            )
            .unwrap();

            let result = candidates(&conn, vec![path.clone()], force).unwrap();

            assert_eq!(result.paths.is_empty(), skipped, "{name}");
            assert_eq!(result.ignored, usize::from(skipped), "{name}");
        }
    }

    #[test]
    fn current_file_is_skipped_changed_and_new_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.jpg");
        fs::write(&p, b"hello").unwrap();
        let mut sigs = HashMap::new();
        sigs.insert(p.to_string_lossy().into_owned(), sig_for(&p, true));

        // unchanged + complete -> skip
        assert!(!needs_processing(&sigs, &p));

        // size change -> process
        fs::write(&p, b"hello world, now bigger").unwrap();
        assert!(needs_processing(&sigs, &p));

        // a path with no stored row -> process
        let q = dir.path().join("b.jpg");
        fs::write(&q, b"x").unwrap();
        assert!(needs_processing(&sigs, &q));
    }
}
