// File fixtures shared by CLI integration tests and event-indexing unit tests.

use rusqlite::OptionalExtension;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    pub mtime: SystemTime,
}

pub struct TestFile {
    path: PathBuf,
    name: PathBuf,
}

impl TestFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn name(&self) -> &Path {
        &self.name
    }

    pub fn meta(&self) -> FileMeta {
        let meta = std::fs::metadata(&self.path).unwrap();
        FileMeta {
            size: meta.len(),
            mtime: meta.modified().unwrap(),
        }
    }

    pub fn size(&self) -> u64 {
        self.meta().size
    }

    pub fn mtime(&self) -> SystemTime {
        self.meta().mtime
    }

    pub fn remove(&self) {
        std::fs::remove_file(&self.path).unwrap();
    }
}

pub struct FileBuilder {
    file: TestFile,
    bytes: Vec<u8>,
    meta: FileMeta,
}

impl FileBuilder {
    fn new(root: &Path, name: impl AsRef<Path>, bytes: Vec<u8>) -> Self {
        let name = name.as_ref().to_path_buf();
        Self {
            file: TestFile {
                path: root.canonicalize().unwrap().join(&name),
                name,
            },
            bytes,
            // Both formats fit, so a photo can replace metadata without
            // changing its size. Padding never truncates the JPEG fixture.
            meta: FileMeta {
                size: 4096,
                mtime: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            },
        }
    }

    pub fn with_meta(mut self, meta: FileMeta) -> Self {
        self.meta = meta;
        self
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.meta.size = size;
        self
    }

    pub fn with_mtime(mut self, mtime: SystemTime) -> Self {
        self.meta.mtime = mtime;
        self
    }

    /// Builders only prepare data; all filesystem changes happen here.
    pub fn write(mut self) -> TestFile {
        let size = usize::try_from(self.meta.size).unwrap();
        assert!(
            size >= self.bytes.len(),
            "the fixture must fit without truncation"
        );
        self.bytes.resize(size, 0);
        std::fs::create_dir_all(self.file.path.parent().unwrap()).unwrap();
        std::fs::write(&self.file.path, self.bytes).unwrap();
        filetime::set_file_mtime(
            &self.file.path,
            filetime::FileTime::from_system_time(self.meta.mtime),
        )
        .unwrap();
        assert_eq!(
            self.file.meta(),
            self.meta,
            "fixture metadata was not applied"
        );
        self.file
    }
}

pub fn photo(root: &Path, name: impl AsRef<Path>) -> FileBuilder {
    FileBuilder::new(root, name, include_bytes!("../fixtures/tiny.jpg").to_vec())
}

pub fn appledouble(root: &Path, name: impl AsRef<Path>) -> FileBuilder {
    FileBuilder::new(root, name, appledouble_bytes())
}

/// AppleDouble v2 with one 32-byte Finder-info entry.
pub fn appledouble_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x0005_1607u32.to_be_bytes());
    bytes.extend_from_slice(&0x0002_0000u32.to_be_bytes());
    bytes.extend_from_slice(&[0; 16]);
    bytes.extend_from_slice(&1u16.to_be_bytes());
    bytes.extend_from_slice(&9u32.to_be_bytes());
    bytes.extend_from_slice(&38u32.to_be_bytes());
    bytes.extend_from_slice(&32u32.to_be_bytes());
    bytes.extend_from_slice(&[0; 32]);
    bytes
}

pub fn assert_ignored(conn: &rusqlite::Connection, file: &TestFile) {
    let reason: Option<String> = conn
        .query_row(
            "SELECT reason FROM ignored_files WHERE path = ?1",
            [file.path.to_str().unwrap()],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(
        reason.as_deref(),
        Some("appledouble"),
        "{}",
        file.path.display()
    );
}

pub fn assert_not_ignored(conn: &rusqlite::Connection, file: &TestFile) {
    assert_absent(conn, "ignored_files", file);
}

pub fn assert_photo(conn: &rusqlite::Connection, file: &TestFile) {
    let mime: Option<String> = conn
        .query_row(
            "SELECT mime FROM file_hashes WHERE path = ?1",
            [file.path.to_str().unwrap()],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    assert_eq!(
        mime.as_deref(),
        Some("image/jpeg"),
        "{}",
        file.path.display()
    );
}

pub fn assert_not_indexed(conn: &rusqlite::Connection, file: &TestFile) {
    assert_absent(conn, "file_hashes", file);
}

fn assert_absent(conn: &rusqlite::Connection, table: &str, file: &TestFile) {
    let count: i64 = conn
        .query_row(
            &format!("SELECT count(*) FROM {table} WHERE path = ?1"),
            [file.path.to_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "{} still contains {}", table, file.path.display());
}
