//! Bringing a known list of files into the library: what `watch` does for the
//! files an event named, and what `dedupe undo` does for the files it put
//! back.

use std::path::PathBuf;

/// Hash those of `paths` that need it (new or changed since their row was
/// written), store their rows, and reconcile their XMP sidecars. Returns the
/// paths written.
pub fn index_paths(
    conn: &rusqlite::Connection,
    library: &videre_core::library::LibraryContext,
    paths: Vec<PathBuf>,
    prec: videre_core::marks::XmpPrecedence,
    silent: bool,
) -> anyhow::Result<Vec<PathBuf>> {
    use rayon::prelude::*;
    let sigs = videre_core::db::stored_signatures(conn).unwrap_or_default();
    let paths: Vec<_> = paths
        .into_iter()
        .filter(|p| videre::incremental::needs_processing(&sigs, p))
        .collect();
    let records: Vec<videre::types::FileRecord> = paths
        .par_iter()
        .filter_map(|path| videre::hasher::hash_file(path).ok())
        .collect();
    videre::sqlite_output::write_records_in(conn, library, &records)?;
    videre::sqlite_output::resolve_unresolved(conn)?;
    videre::sqlite_output::orient_unoriented(conn)?;
    let changed: std::collections::HashSet<String> =
        records.iter().map(|r| r.path.clone()).collect();
    crate::xmp::reconcile_xmp_in(conn, library, prec, &changed, silent)?;
    Ok(records.iter().map(|r| PathBuf::from(&r.path)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use videre_core::library::LibraryContext;

    fn test_library() -> (tempfile::TempDir, LibraryContext, rusqlite::Connection) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("fotoğraflar");
        std::fs::create_dir(&root).unwrap();
        let library = LibraryContext::new(&root, &temp.path().join("cache")).unwrap();
        let conn = videre_core::library_db::initialize(&library).unwrap();
        (temp, library, conn)
    }

    fn photo(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        image::RgbImage::new(2, 3).save(&path).unwrap();
        path
    }

    fn appledouble(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        let mut metadata = [0; 26];
        metadata[..8].copy_from_slice(b"\x00\x05\x16\x07\x00\x02\x00\x00");
        std::fs::write(&path, metadata).unwrap();
        path
    }

    #[test]
    fn event_indexing_excludes_appledouble_but_keeps_a_real_dot_underscore_photo() {
        let (_temp, library, conn) = test_library();
        let root = &library.paths.root;
        let real = photo(root, "._gerçek.jpg");
        let sidecar = appledouble(root, "._çiçek.jpg");

        let written = index_paths(
            &conn,
            &library,
            vec![real.clone(), sidecar],
            videre_core::marks::XmpPrecedence::Db,
            true,
        )
        .unwrap();

        assert_eq!(written, [real]);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM file_hashes", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn index_paths_stores_rows_for_exactly_the_given_files() {
        let (_temp, library, conn) = test_library();
        let root = &library.paths.root;
        let given = [root.join("çiçek.jpg"), root.join("Fotoğraf 1.jpg")];
        let other = root.join("dokunulmadı.jpg");
        for (i, p) in given.iter().chain([&other]).enumerate() {
            std::fs::write(p, format!("bayt {i}")).unwrap();
        }

        let written = index_paths(
            &conn,
            &library,
            given.to_vec(),
            videre_core::marks::XmpPrecedence::Db,
            true,
        )
        .unwrap();

        assert_eq!(written.len(), 2);
        let stored: Vec<String> = conn
            .prepare("SELECT path FROM file_hashes ORDER BY path")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let mut want: Vec<String> = given.iter().map(|p| p.to_string_lossy().into()).collect();
        want.sort();
        assert_eq!(stored, want);
    }
}
