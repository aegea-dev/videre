//! Cached content exclusions, kept outside the media index. A missing table
//! is an empty cache, so older libraries and older readers remain usable.

use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

pub const APPLEDOUBLE: &str = "appledouble";

#[derive(Clone, Debug)]
pub struct IgnoredFile {
    pub path: String,
    pub size_bytes: u64,
    pub modified_at: Option<String>,
    pub reason: String,
}

pub fn ensure_table(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ignored_files (
            path TEXT PRIMARY KEY,
            size_bytes INTEGER NOT NULL,
            modified_at TEXT,
            reason TEXT NOT NULL
        );",
    )
}

/// Only exclusions this version understands may suppress a probe. New reasons
/// written by a newer version are harmless cache misses to an older reader.
pub fn stored_signatures(conn: &Connection) -> rusqlite::Result<HashMap<String, IgnoredFile>> {
    if !crate::db::table_exists(conn, "ignored_files")? {
        return Ok(HashMap::new());
    }
    let mut stmt = conn.prepare(
        "SELECT path, size_bytes, modified_at, reason FROM ignored_files WHERE reason = ?1",
    )?;
    let rows = stmt.query_map([APPLEDOUBLE], |row| {
        let entry = IgnoredFile {
            path: row.get(0)?,
            size_bytes: row.get::<_, i64>(1)? as u64,
            modified_at: row.get(2)?,
            reason: row.get(3)?,
        };
        Ok((entry.path.clone(), entry))
    })?;
    rows.collect()
}

/// Called inside the scan writer's transaction, alongside media writes and
/// removal of exclusions for files now recognized as media.
pub fn write(conn: &Connection, entries: &[IgnoredFile]) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO ignored_files(path, size_bytes, modified_at, reason)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(path) DO UPDATE SET size_bytes = excluded.size_bytes,
             modified_at = excluded.modified_at, reason = excluded.reason",
    )?;
    for entry in entries {
        stmt.execute(params![
            entry.path,
            entry.size_bytes as i64,
            entry.modified_at,
            entry.reason
        ])?;
    }
    Ok(())
}

pub fn forget<'a>(
    conn: &Connection,
    paths: impl IntoIterator<Item = &'a str>,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("DELETE FROM ignored_files WHERE path = ?1")?;
    for path in paths {
        stmt.execute([path])?;
    }
    Ok(())
}

/// A full scan already knows which paths it saw. Drop obsolete cache entries
/// without another filesystem pass. A partial walk can only lose cache hits;
/// it never removes anything from the media index.
pub fn retain_seen(conn: &Connection, seen: &HashSet<String>) -> rusqlite::Result<()> {
    let obsolete: Vec<String> = {
        let mut stmt = conn.prepare("SELECT path FROM ignored_files")?;
        let paths = stmt.query_map([], |row| row.get::<_, String>(0))?;
        paths
            .filter_map(|path| match path {
                Ok(path) if seen.contains(&path) => None,
                other => Some(other),
            })
            .collect::<rusqlite::Result<_>>()?
    };
    let tx = conn.unchecked_transaction()?;
    forget(&tx, obsolete.iter().map(String::as_str))?;
    tx.commit()
}
