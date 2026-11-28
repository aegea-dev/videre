use crate::common::{files, TestLibrary};
use std::path::Path;

impl TestLibrary {
    pub fn photo(&self, name: impl AsRef<Path>) -> files::FileBuilder {
        files::photo(&self.root, name)
    }

    pub fn appledouble(&self, name: impl AsRef<Path>) -> files::FileBuilder {
        files::appledouble(&self.root, name)
    }

    pub fn scan_ok(&self) -> ScanOutput {
        self.scan_with_args(&[])
    }

    pub fn scan_force_ok(&self) -> ScanOutput {
        self.scan_with_args(&["--force"])
    }

    fn scan_with_args(&self, args: &[&str]) -> ScanOutput {
        let output = self.cmd().arg("scan").args(args).output().unwrap();
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(output.status.success(), "{stderr}");
        ScanOutput { stderr }
    }

    pub fn assert_ignored(&self, file: &files::TestFile) {
        files::assert_ignored(&self.conn(), file);
    }

    pub fn assert_not_ignored(&self, file: &files::TestFile) {
        files::assert_not_ignored(&self.conn(), file);
    }

    pub fn assert_photo(&self, file: &files::TestFile) {
        files::assert_photo(&self.conn(), file);
    }

    pub fn assert_not_indexed(&self, file: &files::TestFile) {
        files::assert_not_indexed(&self.conn(), file);
    }

    pub fn drop_ignore_cache(&self) {
        self.conn()
            .execute_batch("DROP TABLE ignored_files")
            .unwrap();
    }

    pub fn assert_ignore_cache_empty(&self) {
        let count: i64 = self
            .conn()
            .query_row("SELECT count(*) FROM ignored_files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}

pub struct ScanOutput {
    stderr: String,
}

impl ScanOutput {
    pub fn assert_written(&self, count: usize) -> &Self {
        assert!(
            self.stderr.contains(&format!("Wrote {count} record(s)")),
            "{}",
            self.stderr
        );
        self
    }

    pub fn assert_skipped(&self, count: usize) -> &Self {
        assert!(
            self.stderr.contains(&format!("({count} skipped)")),
            "{}",
            self.stderr
        );
        self
    }
}
