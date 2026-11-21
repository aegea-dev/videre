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
        .filter_map(|path| videre::hasher::hash_file_in(library, path).ok())
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
        image::RgbImage::from_pixel(2, 3, image::Rgb([255, 0, 0]))
            .save(&path)
            .unwrap();
        path
    }

    fn symlink(root: &Path, name: &str, target: &Path) -> PathBuf {
        let path = root.join(name);
        std::os::unix::fs::symlink(target, &path).unwrap();
        path
    }

    fn assert_indexed_paths(conn: &rusqlite::Connection, expected: &[PathBuf]) {
        let stored: Vec<String> = conn
            .prepare("SELECT path FROM file_hashes ORDER BY path")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        let mut expected: Vec<String> = expected
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        expected.sort();
        assert_eq!(stored, expected);
    }

    #[test]
    fn index_paths_outside_symlink_does_not_block_other_files() {
        let (temp, library, conn) = test_library();
        let root = &library.paths.root;
        let first = photo(root, "çiçek.jpg");
        let second = photo(root, "deniz.jpg");
        let outside = photo(temp.path(), "dışarıdaki.jpg");
        let link = symlink(root, "dışarıya-bağlantı.jpg", &outside);

        let written = index_paths(
            &conn,
            &library,
            vec![first.clone(), link, second.clone()],
            videre_core::marks::XmpPrecedence::Db,
            true,
        )
        .unwrap();

        let expected = [first, second];
        assert_eq!(written, expected);
        assert_indexed_paths(&conn, &expected);
    }

    #[test]
    fn index_paths_replaced_root_does_not_write_foreign_files() {
        let (temp, library, conn) = test_library();

        // Model a root replaced after the caller's check, but before indexing.
        // Keep the context and connection that belong to the original library.
        library.ensure_root_identity().unwrap();
        std::fs::rename(&library.paths.root, temp.path().join("taşınan")).unwrap();
        std::fs::create_dir(&library.paths.root).unwrap();
        let foreign = photo(&library.paths.root, "başka-koleksiyon.jpg");

        let written = index_paths(
            &conn,
            &library,
            vec![foreign],
            videre_core::marks::XmpPrecedence::Db,
            true,
        );

        assert_indexed_paths(&conn, &[]);
        assert!(written.unwrap().is_empty());
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
        assert_indexed_paths(&conn, &given);
    }
}
