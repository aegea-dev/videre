//! Facade over videre's faces-labeling operations. Plain functions over an
//! open `rusqlite::Connection`, returning serde types and a shared `Error`.
//! Called by the gallery server, XMP readback and any other embedder.

use crate::error::{Error, Result};
use crate::types::*;
use rusqlite::Connection;
use std::collections::{BTreeSet, HashMap};

#[derive(Debug)]
struct FaceState {
    id: i64,
    cluster_id: Option<i64>,
    person_label: Option<String>,
    confirmed: bool,
}

fn immediate_transaction<T>(conn: &Connection, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match operation() {
        Ok(value) => match conn.execute_batch("COMMIT") {
            Ok(()) => Ok(value),
            Err(error) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(error.into())
            }
        },
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn face_states(conn: &Connection, face_ids: &[i64]) -> Result<Vec<FaceState>> {
    if face_ids.is_empty() {
        return Err(Error::Invalid);
    }
    let mut seen = BTreeSet::new();
    if let Some(repeat) = face_ids.iter().find(|id| !seen.insert(**id)) {
        return Err(Error::Rejected(format!(
            "the request lists face {repeat} more than once"
        )));
    }
    let mut ids = face_ids.to_vec();
    ids.sort_unstable();
    let mut statement =
        conn.prepare("SELECT cluster_id, person_label, confirmed FROM faces WHERE id = ?1")?;
    ids.into_iter()
        .map(|id| {
            statement
                .query_row([id], |row| {
                    Ok(FaceState {
                        id,
                        cluster_id: row.get(0)?,
                        person_label: row.get(1)?,
                        confirmed: row.get::<_, i64>(2)? != 0,
                    })
                })
                .map_err(|error| match error {
                    rusqlite::Error::QueryReturnedNoRows => Error::NotFound,
                    other => other.into(),
                })
        })
        .collect()
}

fn unassigned_cluster_ids(conn: &Connection, cluster_id: i64) -> Result<Vec<i64>> {
    let mut statement = conn.prepare(
        "SELECT id FROM faces
         WHERE cluster_id = ?1 AND confirmed = 0 AND person_label IS NULL
         ORDER BY id",
    )?;
    let ids = statement
        .query_map([cluster_id], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Whether detection has ever run against this library.
fn faces_table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='faces'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// People / unassigned clusters / singletons for the labeling page.
///
/// :warning: **A library that has never run `videre faces` has no `faces` table
/// at all**, and that is not an error. `videre scan` creates `file_hashes`,
/// `people` and `pipeline_runs`; the faces table arrives with the first
/// detection run. Querying it before then failed with "no such table", which the
/// server turned into a 500 with an empty body, which the page turned into
/// `Unexpected end of JSON input` across the top of the labeling UI.
///
/// "Nothing detected yet" is a state, not a failure. It returns empty here and
/// the page says so.
pub fn faces_list(conn: &Connection) -> Result<FacesData> {
    faces_list_page(conn, Page::ALL, &|_| true)
}

/// `faces_list` with one page of the singles: those whose hash `keep`
/// accepts, after `page.after` by face id. People and clusters always come
/// whole (hundreds at most, and every one is a drop target), while a library
/// can hold tens of thousands of singles. `singles_total` counts every single
/// `keep` accepts, so the page can say how many there are.
pub fn faces_list_page(
    conn: &Connection,
    page: Page,
    keep: &dyn Fn(&str) -> bool,
) -> Result<FacesData> {
    if !faces_table_exists(conn) {
        return Ok(FacesData::default());
    }
    let mut people: HashMap<String, PersonData> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            // LEFT JOIN, not JOIN: a face labelled before the people table
            // existed still has to appear, showing its raw label until the
            // migration gives it a row.
            "SELECT f.id, f.hash, f.person_label, COALESCE(p.full_name, f.person_label) \
             FROM faces f LEFT JOIN people p ON p.name = f.person_label \
             WHERE f.confirmed = 1 AND f.person_label IS NOT NULL \
             ORDER BY f.person_label, f.is_primary DESC, f.id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (id, hash, label, full_name) = row?;
            let person = people.entry(label.clone()).or_insert(PersonData {
                label: label.clone(),
                full_name,
                face_ids: vec![],
                representative_id: id,
                hashes: vec![],
            });
            person.face_ids.push(id);
            if !person.hashes.contains(&hash) {
                person.hashes.push(hash);
            }
        }
    }

    let mut cluster_map: HashMap<i64, ClusterData> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, hash, cluster_id FROM faces \
             WHERE cluster_id IS NOT NULL AND (confirmed = 0 OR person_label IS NULL) \
             ORDER BY cluster_id, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        for row in rows {
            let (id, hash, cid) = row?;
            let cluster = cluster_map.entry(cid).or_insert(ClusterData {
                cluster_id: cid,
                face_ids: vec![],
                hashes: vec![],
            });
            cluster.face_ids.push(id);
            if !cluster.hashes.contains(&hash) {
                cluster.hashes.push(hash);
            }
        }
    }

    let mut singletons: Vec<SingletonData> = vec![];
    {
        let mut stmt = conn.prepare(
            "SELECT id, hash FROM faces \
             WHERE cluster_id IS NULL AND (confirmed = 0 OR person_label IS NULL) \
             ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, hash) = row?;
            if keep(&hash) {
                singletons.push(SingletonData { face_id: id, hash });
            }
        }
    }
    let singles_total = singletons.len();
    let (singletons, singles_next) = take_page(
        singletons
            .into_iter()
            .filter(|s| page.after.is_none_or(|after| s.face_id > after)),
        page.limit,
        |s| s.face_id,
    );

    // Both maps are HashMaps, whose iteration order is arbitrary and differs
    // between instances, so collecting straight from them threw away the
    // ORDER BY the queries above establish. The labeling UI re-fetches this
    // list after every assignment, so the effect was that people and clusters
    // reshuffled on each drop: the cluster lined up next moved somewhere else,
    // and so did the person being dragged onto. `singletons` never had the
    // problem, and the difference is exactly that it is built as a Vec.
    //
    // Clusters are ordered largest first, which is the order people label in:
    // the big clusters are worth the most and are the easiest to recognise.
    // cluster_id breaks ties so the order is total, not merely sorted.
    let mut people: Vec<PersonData> = people.into_values().collect();
    people.sort_by_key(|a| a.full_name.to_lowercase());
    let mut clusters: Vec<ClusterData> = cluster_map.into_values().collect();
    clusters.sort_by(|a, b| {
        b.face_ids
            .len()
            .cmp(&a.face_ids.len())
            .then(a.cluster_id.cmp(&b.cluster_id))
    });

    Ok(FacesData {
        people,
        clusters,
        singletons,
        singles_total,
        singles_next,
    })
}

/// At most `limit` of `items`, and the id to page after when any remain.
fn take_page<T>(
    items: impl Iterator<Item = T>,
    limit: usize,
    id: impl Fn(&T) -> i64,
) -> (Vec<T>, Option<i64>) {
    let mut items = items.peekable();
    let page: Vec<T> = items.by_ref().take(limit).collect();
    let next = match (items.peek(), page.last()) {
        (Some(_), Some(last)) => Some(id(last)),
        _ => None,
    };
    (page, next)
}

/// Every face in one unassigned cluster (for the cluster detail page).
pub fn cluster_detail(conn: &Connection, cluster_id: i64) -> Result<ClusterDetail> {
    // Same unlabeled filter the cluster card uses: the page a card opens
    // must show the population the card counted. A labeled face can hold no
    // cluster id any more; the filter stays so the two queries cannot drift.
    // One path per face: a photo with two identical copies has one set of
    // faces, and listing each twice made the page post ids the cluster check
    // rejects.
    let mut stmt = conn.prepare(
        "SELECT f.id, f.hash, MIN(fh.path) FROM faces f \
         JOIN file_hashes fh ON f.hash = fh.hash \
         WHERE f.cluster_id = ?1 AND (f.confirmed = 0 OR f.person_label IS NULL) \
         GROUP BY f.id \
         ORDER BY f.id",
    )?;
    let faces = stmt
        .query_map([cluster_id], |r| {
            Ok(ClusterFaceData {
                face_id: r.get(0)?,
                hash: r.get(1)?,
                path: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ClusterDetail { cluster_id, faces })
}

/// Every confirmed face for one person, primary first and flagged.
pub fn person_detail(conn: &Connection, name: &str) -> Result<PersonDetail> {
    person_detail_page(conn, name, Page::ALL)
}

/// One page of a person's confirmed faces: the primary first, then by id.
/// Paging after the primary starts the rest from the lowest id; after any
/// other face, from the next id above it, so a page holds when an earlier
/// face is removed.
pub fn person_detail_page(conn: &Connection, name: &str, page: Page) -> Result<PersonDetail> {
    // Reads normalize too, so `/people/person/Erhan`, `/people/person/erhan` and the original
    // spelling all reach the same person. That is what keeps existing links
    // working across the migration without a redirect table.
    let name = videre_core::person::normalize(name).unwrap_or_else(|| name.to_string());
    let name = name.as_str();
    // One path per face, as in `cluster_detail`.
    let mut stmt = conn.prepare(
        "SELECT f.id, f.hash, MIN(fh.path), f.is_primary FROM faces f \
         JOIN file_hashes fh ON f.hash = fh.hash \
         WHERE f.person_label = ?1 AND f.confirmed = 1 \
         GROUP BY f.id \
         ORDER BY f.is_primary DESC, f.id",
    )?;
    let faces = stmt
        .query_map([name], |r| {
            Ok(PersonFaceData {
                face_id: r.get(0)?,
                hash: r.get(1)?,
                path: r.get(2)?,
                is_primary: r.get::<_, i64>(3)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let face_total = faces.len();
    let primary = faces.first().filter(|f| f.is_primary).map(|f| f.face_id);
    let (faces, next) = take_page(
        faces.into_iter().filter(|f| match page.after {
            None => true,
            Some(after) if Some(after) == primary => !f.is_primary,
            Some(after) => !f.is_primary && f.face_id > after,
        }),
        page.limit,
        |f| f.face_id,
    );
    // Falls back to the identity for a person with no row yet, so a library
    // opened before the migration still shows something sensible.
    let full_name: String = conn
        .query_row(
            "SELECT full_name FROM people WHERE name = ?1",
            rusqlite::params![name],
            |r| r.get(0),
        )
        .unwrap_or_else(|_| name.to_string());
    Ok(PersonDetail {
        label: name.to_string(),
        full_name,
        faces,
        face_total,
        next,
    })
}

/// Image paths for confirmed faces of a person, matched by identity or by
/// display name (`person::resolve_identities`), never by prefix. Delegates to
/// the existing core search.
pub fn search_person(conn: &Connection, name: &str) -> Result<Vec<String>> {
    Ok(videre_core::person_search::search_by_person(
        conn, name, None,
    )?)
}

/// Assign faces to an existing/new person: sets person_label + confirmed.
/// Rejects an empty label after sanitizing.
pub fn assign(conn: &Connection, face_ids: &[i64], person_label: &str) -> Result<()> {
    // What was typed becomes the display name; its normalized form is the
    // identity written to every face row. Upserting keeps `people` complete
    // without a separate "create person" step.
    let display = crate::label::sanitize_person_label(person_label).ok_or(Error::Invalid)?;
    let label = videre_core::person::normalize(&display).ok_or(Error::Invalid)?;
    // Nothing to assign is a malformed request, not a silent success that would
    // create a person with no faces.
    if face_ids.is_empty() {
        return Err(Error::Invalid);
    }
    // All-or-nothing: a face id that matches no row makes the whole assign a
    // NotFound, and the person insert is rolled back with it so a failed assign
    // leaves nothing behind. An `UPDATE` matching no row is `Ok(0)`, not an
    // error, so a partial write would otherwise be reported as success.
    conn.execute_batch("BEGIN")?;
    let result = assign_in_transaction(conn, face_ids, &label, &display);
    finish_unit_transaction(conn, result)
}

fn finish_unit_transaction(conn: &Connection, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => {
            if let Err(error) = conn.execute_batch("COMMIT") {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(error.into());
            }
            Ok(())
        }
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(error)
        }
    }
}

fn assign_in_transaction(
    conn: &Connection,
    face_ids: &[i64],
    identity: &str,
    display: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO people (name, full_name) VALUES (?1, ?2) ON CONFLICT(name) DO NOTHING",
        rusqlite::params![identity, display],
    )?;
    for id in face_ids {
        let changed = conn.execute(
            "UPDATE faces
             SET person_label = ?1, confirmed = 1, cluster_id = NULL
             WHERE id = ?2",
            rusqlite::params![identity, id],
        )?;
        if changed == 0 {
            return Err(Error::NotFound);
        }
    }
    Ok(())
}

fn validate_subject(conn: &Connection, face_ids: &[i64]) -> Result<Vec<FaceState>> {
    let states = face_states(conn, face_ids)?;
    if let Some(state) = states
        .iter()
        .find(|state| state.confirmed || state.person_label.is_some())
    {
        return Err(Error::Rejected(format!(
            "face {} is already named or confirmed",
            state.id
        )));
    }
    if let (1, Some(cluster_id)) = (states.len(), states[0].cluster_id) {
        return Err(Error::Rejected(format!(
            "face {} belongs to cluster {cluster_id}; assign the cluster or remove the face from it first",
            states[0].id
        )));
    }
    if states.len() > 1 {
        let cluster_id = states[0]
            .cluster_id
            .ok_or_else(|| Error::Rejected("the faces are not in a cluster".into()))?;
        let members = unassigned_cluster_ids(conn, cluster_id)?;
        if states
            .iter()
            .any(|state| state.cluster_id != Some(cluster_id))
            || members != states.iter().map(|state| state.id).collect::<Vec<_>>()
        {
            return Err(Error::Rejected(format!(
                "the request lists {} face(s), cluster {cluster_id} has {} unassigned face(s)",
                states.len(),
                members.len()
            )));
        }
    }
    Ok(states)
}

fn person_exists(conn: &Connection, identity: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM people WHERE name = ?1)",
        [identity],
        |row| row.get(0),
    )?)
}

/// Name faces as a person who already exists, the way the gallery does: the
/// faces must be unnamed, and several faces must be one cluster's whole
/// unassigned group. Unlike `assign`, it never creates the person.
pub fn assign_to_person(conn: &Connection, face_ids: &[i64], person_label: &str) -> Result<()> {
    let display = crate::label::sanitize_person_label(person_label).ok_or(Error::Invalid)?;
    let identity = videre_core::person::normalize(&display).ok_or(Error::Invalid)?;
    immediate_transaction(conn, || {
        validate_subject(conn, face_ids)?;
        if !person_exists(conn, &identity)? {
            return Err(Error::NotFound);
        }
        assign_in_transaction(conn, face_ids, &identity, &display)
    })
}

/// Name faces as a new person, under the rules of `assign_to_person`. A name
/// that already exists joins that person.
pub fn new_person(conn: &Connection, face_ids: &[i64], person_label: &str) -> Result<()> {
    let display = crate::label::sanitize_person_label(person_label).ok_or(Error::Invalid)?;
    let identity = videre_core::person::normalize(&display).ok_or(Error::Invalid)?;
    immediate_transaction(conn, || {
        validate_subject(conn, face_ids)?;
        assign_in_transaction(conn, face_ids, &identity, &display)
    })
}

/// Reset one face to fully unassigned (cluster, label, confirmed, primary).
/// Only a named face outside a cluster, or an unnamed face in one, can be
/// taken off: anything else is not a state the gallery shows.
pub fn remove_face(conn: &Connection, face_id: i64) -> Result<()> {
    immediate_transaction(conn, || {
        let state = face_states(conn, &[face_id])?.remove(0);
        let named = state.confirmed && state.person_label.is_some() && state.cluster_id.is_none();
        let clustered =
            !state.confirmed && state.person_label.is_none() && state.cluster_id.is_some();
        if !(named || clustered) {
            return Err(Error::Invalid);
        }
        remove_face_in_transaction(conn, face_id)
    })
}

fn remove_face_in_transaction(conn: &Connection, face_id: i64) -> Result<()> {
    let n = conn.execute(
        "UPDATE faces SET cluster_id = NULL, person_label = NULL, confirmed = 0, is_primary = 0 WHERE id = ?1",
        [face_id],
    )?;
    if n == 0 {
        return Err(Error::NotFound);
    }
    Ok(())
}

/// Ungroup a bad cluster: its faces become unassigned singletons (not
/// deleted). Every face in it must be unassigned, and a cluster is at least
/// two faces.
pub fn dissolve_cluster(conn: &Connection, cluster_id: i64) -> Result<()> {
    immediate_transaction(conn, || {
        let face_ids = unassigned_cluster_ids(conn, cluster_id)?;
        let all_faces: i64 = conn.query_row(
            "SELECT COUNT(*) FROM faces WHERE cluster_id = ?1",
            [cluster_id],
            |row| row.get(0),
        )?;
        if all_faces != face_ids.len() as i64 {
            return Err(Error::Invalid);
        }
        match face_ids.len() {
            0 => return Err(Error::NotFound),
            1 => return Err(Error::Invalid),
            _ => {}
        }
        dissolve_cluster_in_transaction(conn, cluster_id)
    })
}

fn dissolve_cluster_in_transaction(conn: &Connection, cluster_id: i64) -> Result<()> {
    let n = conn.execute(
        "UPDATE faces SET cluster_id = NULL WHERE cluster_id = ?1",
        [cluster_id],
    )?;
    if n == 0 {
        return Err(Error::NotFound);
    }
    Ok(())
}

/// Reset every face of a person back to unassigned. Deliberately does NOT touch
/// cluster_id, so a face rejoins its cluster's unassigned group rather than
/// scattering to singletons.
/// Change only what a person is shown as, never their identity.
///
/// This is the only rename there is. Identity is permanent: `Erhan` to
/// `Erhan Gündoğan` is a display correction even though its normalized form
/// would change too, and there is no way to ask for the other reading. One row,
/// no face touched, and `/people/person/<name>` keeps working, which is the whole
/// reason identity and display are separate.
pub fn set_full_name(conn: &Connection, name: &str, full_name: &str) -> Result<()> {
    let display = crate::label::sanitize_person_label(full_name).ok_or(Error::Invalid)?;
    let name = videre_core::person::normalize(name).ok_or(Error::Invalid)?;
    let n = conn.execute(
        "UPDATE people SET full_name = ?1 WHERE name = ?2",
        rusqlite::params![display, name],
    )?;
    if n == 0 {
        return Err(Error::NotFound);
    }
    Ok(())
}

pub fn delete_person(conn: &Connection, label: &str) -> Result<()> {
    let label = videre_core::person::normalize(label).unwrap_or_else(|| label.to_string());
    // One transaction: the faces either come back unassigned AND the
    // regroup gate reopens, or nothing changes at all.
    conn.execute_batch("BEGIN")?;
    let result = delete_person_in_transaction(conn, &label).map(|_| ());
    finish_unit_transaction(conn, result)
}

fn delete_person_in_transaction(conn: &Connection, identity: &str) -> Result<usize> {
    let changed = conn.execute(
        "UPDATE faces
         SET person_label = NULL, confirmed = 0, is_primary = 0, cluster_id = NULL
         WHERE person_label = ?1",
        [identity],
    )?;
    if changed > 0 {
        videre_core::library_state::set(
            conn,
            videre_core::library_state::FACE_RECLUSTER_WATERMARK,
            0,
        )?;
    }
    Ok(changed)
}

pub fn set_primary(conn: &Connection, face_id: i64, person_label: &str) -> Result<()> {
    let person_label =
        videre_core::person::normalize(person_label).unwrap_or_else(|| person_label.to_string());
    conn.execute_batch("BEGIN")?;
    let result = (|| -> Result<()> {
        conn.execute(
            "UPDATE faces SET is_primary = 0 WHERE person_label = ?1",
            rusqlite::params![person_label],
        )?;
        // The guard on person_label means a face id that does not exist, or
        // belongs to someone else, matches no row: `Ok(0)`, not an error. That
        // is a NotFound, and the rollback restores the primary cleared above so
        // a failed call leaves the person's primary untouched.
        let n = conn.execute(
            "UPDATE faces SET is_primary = 1, confirmed = 1, person_label = ?1 WHERE id = ?2 AND person_label = ?1",
            rusqlite::params![person_label, face_id],
        )?;
        if n == 0 {
            return Err(Error::NotFound);
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_detaches_the_face_from_its_cluster() {
        let conn = seed();
        // Face 3 sits in cluster 7 (unassigned). Assigning it to a person
        // must detach the machine grouping in the same write.
        assign(&conn, &[3], "Bob").unwrap();
        let (label, confirmed, cid): (Option<String>, i64, Option<i64>) = conn
            .query_row(
                "SELECT person_label, confirmed, cluster_id FROM faces WHERE id = 3",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(label.as_deref(), Some("bob"));
        assert_eq!(confirmed, 1);
        assert_eq!(cid, None, "assignment must detach the machine grouping");
    }

    #[test]
    fn cluster_detail_never_shows_labeled_faces() {
        let conn = seed();
        // A tombstone the detach migration should have cleared: a labeled
        // face still carrying a cluster id. The filter keeps the detail
        // page from ever showing it, whatever wrote that row.
        conn.execute(
            "INSERT INTO faces (id,hash,bbox,embedding,cluster_id,person_label,confirmed) VALUES
                (11,'h6','0,0,9,9',X'0000',7,'alice',1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO file_hashes (hash, path) VALUES ('h6','/p/6.jpg')",
            [],
        )
        .unwrap();
        let detail = cluster_detail(&conn, 7).unwrap();
        assert_eq!(
            detail.faces.len(),
            2,
            "only the unlabeled faces of cluster 7 belong on the page"
        );
    }

    /// In-memory db with the faces + file_hashes tables and a few rows:
    /// - face 1: person "Alice", confirmed, is_primary
    /// - face 2: person "Alice", confirmed
    /// - face 3: cluster 7 (unassigned)
    /// - face 4: cluster 7 (unassigned)
    /// - face 5: singleton (no cluster, unassigned)
    pub(super) fn seed() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        videre_core::face_db::create_faces_table(&conn).unwrap();
        conn.execute_batch(
            "CREATE TABLE file_hashes (hash TEXT PRIMARY KEY, path TEXT);
             INSERT INTO file_hashes VALUES ('h1','/p/1.jpg'),('h2','/p/2.jpg'),
                ('h3','/p/3.jpg'),('h4','/p/4.jpg'),('h5','/p/5.jpg');
             -- Labels are stored in identity form, as `assign` writes them and
             -- as the migration leaves them; `people` carries what a reader
             -- sees. Seeding raw 'Alice' would test a state the application no
             -- longer produces.
             INSERT INTO people (name, full_name) VALUES ('alice','Alice');
             INSERT INTO faces (id,hash,bbox,embedding,cluster_id,person_label,confirmed,is_primary) VALUES
                (1,'h1','0,0,9,9',X'0000',NULL,'alice',1,1),
                (2,'h2','0,0,9,9',X'0000',NULL,'alice',1,0),
                (3,'h3','0,0,9,9',X'0000',7,NULL,0,0),
                (4,'h4','0,0,9,9',X'0000',7,NULL,0,0),
                (5,'h5','0,0,9,9',X'0000',NULL,NULL,0,0);",
        )
        .unwrap();
        videre_core::library_db::ensure_scan_schema(&conn).unwrap();
        conn
    }

    /// The gallery's naming rules: what a request may name, take off or
    /// ungroup, so a click on a stale page cannot write a state the page
    /// never showed.
    mod gallery_rules {
        use super::*;

        fn label_of(conn: &Connection, id: i64) -> (Option<String>, i64, Option<i64>) {
            conn.query_row(
                "SELECT person_label, confirmed, cluster_id FROM faces WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
        }

        #[test]
        fn a_repeated_face_is_rejected_with_a_reason() {
            let conn = seed();
            let err = new_person(&conn, &[3, 3, 4], "Bülent").unwrap_err();
            assert_eq!(err.to_string(), "the request lists face 3 more than once");
        }

        #[test]
        fn a_partial_cluster_is_rejected_with_the_counts() {
            let conn = seed();
            conn.execute(
                "INSERT INTO faces (id,hash,bbox,embedding,cluster_id) VALUES (6,'h5','1,1,9,9',X'0000',7)",
                [],
            )
            .unwrap();
            let err = new_person(&conn, &[3, 4], "Bülent").unwrap_err();
            assert_eq!(
                err.to_string(),
                "the request lists 2 face(s), cluster 7 has 3 unassigned face(s)"
            );
        }

        #[test]
        fn a_named_face_is_rejected_with_a_reason() {
            let conn = seed();
            let err = new_person(&conn, &[1], "Bülent").unwrap_err();
            assert_eq!(err.to_string(), "face 1 is already named or confirmed");
        }

        #[test]
        fn one_face_of_a_cluster_is_rejected() {
            let conn = seed();
            let err = assign_to_person(&conn, &[3], "alice").unwrap_err();
            assert_eq!(
                err.to_string(),
                "face 3 belongs to cluster 7; assign the cluster or remove the face from it first"
            );
        }

        #[test]
        fn assigning_to_someone_who_does_not_exist_is_not_found() {
            let conn = seed();
            assert!(matches!(
                assign_to_person(&conn, &[5], "Şükrü"),
                Err(Error::NotFound)
            ));
            assert_eq!(label_of(&conn, 5), (None, 0, None));
        }

        #[test]
        fn a_single_joins_an_existing_person_and_a_new_name_creates_one() {
            let conn = seed();
            assign_to_person(&conn, &[5], "Alice").unwrap();
            assert_eq!(label_of(&conn, 5), (Some("alice".into()), 1, None));
            new_person(&conn, &[3, 4], "Gökçe Yıldız").unwrap();
            assert_eq!(label_of(&conn, 3), (Some("gokce_yildiz".into()), 1, None));
            let full: String = conn
                .query_row(
                    "SELECT full_name FROM people WHERE name = 'gokce_yildiz'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(full, "Gökçe Yıldız");
        }

        #[test]
        fn a_new_person_with_an_existing_name_joins_them() {
            let conn = seed();
            new_person(&conn, &[3, 4], "ALICE").unwrap();
            assert_eq!(label_of(&conn, 3), (Some("alice".into()), 1, None));
        }

        #[test]
        fn only_a_named_or_a_clustered_face_can_be_taken_off() {
            let conn = seed();
            assert!(matches!(remove_face(&conn, 5), Err(Error::Invalid)));
            remove_face(&conn, 2).unwrap();
            assert_eq!(label_of(&conn, 2), (None, 0, None));
            remove_face(&conn, 3).unwrap();
            assert_eq!(label_of(&conn, 3), (None, 0, None));
        }

        #[test]
        fn only_a_whole_unassigned_cluster_of_two_or_more_dissolves() {
            let conn = seed();
            assert!(matches!(dissolve_cluster(&conn, 42), Err(Error::NotFound)));
            conn.execute("UPDATE faces SET cluster_id = 8 WHERE id = 5", [])
                .unwrap();
            assert!(matches!(dissolve_cluster(&conn, 8), Err(Error::Invalid)));
            conn.execute(
                "INSERT INTO faces (id,hash,bbox,embedding,cluster_id,person_label,confirmed)
                 VALUES (6,'h5','1,1,9,9',X'0000',7,'alice',1)",
                [],
            )
            .unwrap();
            assert!(matches!(dissolve_cluster(&conn, 7), Err(Error::Invalid)));
            conn.execute("DELETE FROM faces WHERE id = 6", []).unwrap();
            dissolve_cluster(&conn, 7).unwrap();
            assert_eq!(label_of(&conn, 3), (None, 0, None));
            assert_eq!(label_of(&conn, 4), (None, 0, None));
        }
    }

    #[test]
    fn the_list_comes_back_in_the_same_order_every_time() {
        // The labeling UI re-fetches after every assignment, so an unstable
        // order means the cluster lined up next moves, and so does the person
        // being dragged onto. Both lists were collected straight out of a
        // HashMap, which discarded the ORDER BY in the queries above.
        // `singletons` never had the bug, and the only difference is that it is
        // built as a Vec.
        let conn = seed();
        // The seed has one cluster and one person, which cannot show an
        // ordering problem. Add enough of both to have an order at all, with
        // sizes deliberately not matching id order.
        conn.execute_batch(
            // Columns named explicitly: `seed` runs ensure_scan_schema,
            // so the table has more than the two it was created with.
            "INSERT INTO file_hashes (hash, path) VALUES ('h6','/p/6.jpg'),('h7','/p/7.jpg'),
                ('h8','/p/8.jpg'),('h9','/p/9.jpg'),('h10','/p/10.jpg');
             INSERT INTO people (name, full_name) VALUES ('bob','Bob');
             INSERT INTO faces (id,hash,bbox,embedding,cluster_id,person_label,confirmed,is_primary) VALUES
                (6,'h6','0,0,9,9',X'0000',9,NULL,0,0),
                (7,'h7','0,0,9,9',X'0000',9,NULL,0,0),
                (8,'h8','0,0,9,9',X'0000',9,NULL,0,0),
                (9,'h9','0,0,9,9',X'0000',3,NULL,0,0),
                (10,'h10','0,0,9,9',X'0000',NULL,'bob',1,0);",
        )
        .unwrap();

        // Two calls on one connection: each builds fresh HashMaps, and Rust
        // seeds them differently, so an unstable order shows up here.
        let a = faces_list(&conn).unwrap();
        let b = faces_list(&conn).unwrap();

        let ids = |f: &FacesData| -> Vec<i64> { f.clusters.iter().map(|c| c.cluster_id).collect() };
        let names =
            |f: &FacesData| -> Vec<String> { f.people.iter().map(|p| p.label.clone()).collect() };
        assert!(ids(&a).len() >= 3, "fixture must have several clusters");
        assert_eq!(
            ids(&a),
            ids(&b),
            "cluster order must not change between calls"
        );
        assert_eq!(
            names(&a),
            names(&b),
            "people order must not change between calls"
        );

        // And the order is the useful one: biggest first, so the cluster worth
        // the most labelling effort is where it is expected.
        let sizes: Vec<usize> = a.clusters.iter().map(|c| c.face_ids.len()).collect();
        let mut want = sizes.clone();
        want.sort_unstable_by(|x, y| y.cmp(x));
        assert_eq!(
            sizes, want,
            "clusters must be ordered largest first, got {sizes:?}"
        );
    }

    /// The seed's single (5) plus six more, 11 to 16, so singles page.
    fn seven_singles() -> Connection {
        let conn = seed();
        for id in 11..=16 {
            conn.execute(
                "INSERT INTO file_hashes (hash, path) VALUES (?1, ?2)",
                rusqlite::params![format!("h{id}"), format!("/p/yüz {id}.jpg")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO faces (id,hash,bbox,embedding,cluster_id,person_label,confirmed,is_primary) \
                 VALUES (?1, ?2, '0,0,9,9', X'0000', NULL, NULL, 0, 0)",
                rusqlite::params![id, format!("h{id}")],
            )
            .unwrap();
        }
        conn
    }

    fn single_ids(d: &FacesData) -> Vec<i64> {
        d.singletons.iter().map(|s| s.face_id).collect()
    }

    #[test]
    fn singles_page_by_face_id() {
        let conn = seven_singles();
        let all = |_: &str| true;
        let first = faces_list_page(&conn, Page::first(3), &all).unwrap();
        assert_eq!(single_ids(&first), vec![5, 11, 12]);
        assert_eq!((first.singles_total, first.singles_next), (7, Some(12)));
        // People and clusters stay whole on every page.
        assert_eq!((first.people.len(), first.clusters.len()), (1, 1));

        let second = faces_list_page(&conn, Page::after(12, 3), &all).unwrap();
        assert_eq!(single_ids(&second), vec![13, 14, 15]);
        assert_eq!(second.singles_next, Some(15));
        let last = faces_list_page(&conn, Page::after(15, 3), &all).unwrap();
        assert_eq!(single_ids(&last), vec![16]);
        assert_eq!((last.singles_total, last.singles_next), (7, None));
    }

    #[test]
    fn a_singles_page_holds_after_earlier_singles_are_assigned() {
        let conn = seven_singles();
        let all = |_: &str| true;
        assign(&conn, &[5, 11], "Ayşe").unwrap();
        // The page after 12 is the same faces it was before: keyset, not offset.
        let page = faces_list_page(&conn, Page::after(12, 3), &all).unwrap();
        assert_eq!(single_ids(&page), vec![13, 14, 15]);
        assert_eq!(page.singles_total, 5);
    }

    #[test]
    fn a_filter_narrows_singles_before_paging_and_counting() {
        let conn = seven_singles();
        let keep = |hash: &str| ["h12", "h14", "h16"].contains(&hash);
        let page = faces_list_page(&conn, Page::first(2), &keep).unwrap();
        assert_eq!(single_ids(&page), vec![12, 14]);
        assert_eq!((page.singles_total, page.singles_next), (3, Some(14)));
    }

    #[test]
    fn a_page_of_no_singles_still_carries_people_clusters_and_the_total() {
        let conn = seven_singles();
        let page = faces_list_page(&conn, Page::first(0), &|_: &str| true).unwrap();
        assert!(page.singletons.is_empty());
        assert_eq!((page.people.len(), page.clusters.len()), (1, 1));
        assert_eq!((page.singles_total, page.singles_next), (7, None));
    }

    #[test]
    fn a_person_pages_primary_first_then_by_id() {
        let conn = seed();
        // Alice has 1 (primary) and 2; give her 6, 7 and 8 too.
        for id in 6..=8 {
            conn.execute(
                "INSERT INTO file_hashes (hash, path) VALUES (?1, ?2)",
                rusqlite::params![format!("h{id}"), format!("/p/çiçek {id}.jpg")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO faces (id,hash,bbox,embedding,cluster_id,person_label,confirmed,is_primary) \
                 VALUES (?1, ?2, '0,0,9,9', X'0000', NULL, 'alice', 1, 0)",
                rusqlite::params![id, format!("h{id}")],
            )
            .unwrap();
        }
        set_primary(&conn, 7, "alice").unwrap();
        let ids = |p: &PersonDetail| -> Vec<i64> { p.faces.iter().map(|f| f.face_id).collect() };

        let one = person_detail_page(&conn, "alice", Page::first(2)).unwrap();
        assert_eq!(ids(&one), vec![7, 1]);
        assert!(one.faces[0].is_primary);
        assert_eq!((one.face_total, one.next), (5, Some(1)));
        let two = person_detail_page(&conn, "alice", Page::after(1, 2)).unwrap();
        assert_eq!(ids(&two), vec![2, 6]);
        let three = person_detail_page(&conn, "alice", Page::after(6, 2)).unwrap();
        assert_eq!(ids(&three), vec![8]);
        assert_eq!(three.next, None);
        // A first page of one is the primary alone; the next continues from
        // the lowest id, not from the primary's.
        let only = person_detail_page(&conn, "alice", Page::first(1)).unwrap();
        assert_eq!((ids(&only), only.next), (vec![7], Some(7)));
        let rest = person_detail_page(&conn, "alice", Page::after(7, 10)).unwrap();
        assert_eq!(ids(&rest), vec![1, 2, 6, 8]);
        // The whole list is still one call away.
        assert_eq!(
            ids(&person_detail(&conn, "alice").unwrap()),
            vec![7, 1, 2, 6, 8]
        );
    }

    #[test]
    fn faces_list_splits_people_clusters_singletons() {
        let conn = seed();
        let d = faces_list(&conn).unwrap();
        assert_eq!(d.people.len(), 1);
        // Identity is the normalized form; what a reader sees is separate.
        assert_eq!(d.people[0].label, "alice");
        assert_eq!(d.people[0].full_name, "Alice");
        assert_eq!(
            d.people[0].representative_id, 1,
            "primary face is representative"
        );
        assert_eq!(d.clusters.len(), 1);
        assert_eq!(d.clusters[0].cluster_id, 7);
        assert_eq!(d.clusters[0].face_ids, vec![3, 4]);
        assert_eq!(d.singletons.len(), 1);
        assert_eq!(d.singletons[0].face_id, 5);
    }

    #[test]
    fn person_detail_marks_primary() {
        let conn = seed();
        let p = person_detail(&conn, "Alice").unwrap();
        assert_eq!(p.faces.len(), 2);
        assert!(p.faces[0].is_primary, "primary sorts first and is flagged");
        assert!(!p.faces[1].is_primary);
    }

    /// A photo stored at two byte-identical paths has one set of faces.
    fn seed_with_a_second_path_for(hash: &str) -> Connection {
        let conn = seed();
        conn.execute_batch(
            "ALTER TABLE file_hashes RENAME TO file_hashes_old;
             CREATE TABLE file_hashes (path TEXT PRIMARY KEY, hash TEXT);
             INSERT INTO file_hashes (path, hash) SELECT path, hash FROM file_hashes_old;
             DROP TABLE file_hashes_old;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO file_hashes (path, hash) VALUES (?1, ?2)",
            rusqlite::params![format!("/copy/{hash}.jpg"), hash],
        )
        .unwrap();
        conn
    }

    #[test]
    fn detail_pages_list_a_face_once_when_its_photo_has_two_paths() {
        let conn = seed_with_a_second_path_for("h3");
        let c = cluster_detail(&conn, 7).unwrap();
        assert_eq!(
            c.faces.iter().map(|f| f.face_id).collect::<Vec<_>>(),
            vec![3, 4]
        );
        let conn = seed_with_a_second_path_for("h1");
        let p = person_detail(&conn, "Alice").unwrap();
        assert_eq!(
            p.faces.iter().map(|f| f.face_id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(p.faces[0].is_primary);
    }

    #[test]
    fn cluster_detail_lists_faces() {
        let conn = seed();
        let c = cluster_detail(&conn, 7).unwrap();
        assert_eq!(c.cluster_id, 7);
        assert_eq!(
            c.faces.iter().map(|f| f.face_id).collect::<Vec<_>>(),
            vec![3, 4]
        );
    }

    #[test]
    fn assign_labels_and_confirms() {
        let conn = seed();
        assign(&conn, &[3, 4], "Bob").unwrap();
        let p = person_detail(&conn, "Bob").unwrap();
        assert_eq!(p.faces.len(), 2, "both faces now confirmed under Bob");
    }

    #[test]
    fn assign_rejects_empty_label() {
        let conn = seed();
        assert!(matches!(assign(&conn, &[3], "   "), Err(Error::Invalid)));
    }

    #[test]
    fn remove_face_unassigns_everything() {
        let conn = seed();
        remove_face(&conn, 1).unwrap();
        let (cid, label, confirmed, prim): (Option<i64>, Option<String>, i64, i64) = conn
            .query_row(
                "SELECT cluster_id, person_label, confirmed, is_primary FROM faces WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((cid, label, confirmed, prim), (None, None, 0, 0));
    }

    #[test]
    fn dissolve_cluster_nulls_cluster_id() {
        let conn = seed();
        dissolve_cluster(&conn, 7).unwrap();
        assert_eq!(faces_list(&conn).unwrap().clusters.len(), 0);
        assert_eq!(
            faces_list(&conn).unwrap().singletons.len(),
            3,
            "3,4 join 5 as singletons"
        );
    }

    #[test]
    fn deleting_a_missing_person_leaves_the_regrouping_gate_alone() {
        // A delete that matched zero faces is a no-op by design; it must not
        // schedule a whole-library regroup (watermark reset) for nothing.
        let conn = seed();
        videre_core::face_db::advance_recluster_watermark(&conn).unwrap();
        let before = videre_core::face_db::recluster_watermark(&conn).unwrap();
        assert!(before > 0);
        delete_person(&conn, "ghost").unwrap();
        assert_eq!(
            videre_core::face_db::recluster_watermark(&conn).unwrap(),
            before,
            "a no-op delete must not reopen the gated regroup"
        );
    }

    #[test]
    fn delete_person_returns_faces_to_the_unassigned_pool_and_reopens_regrouping() {
        // The real workflow: assign through the public path (which detaches
        // the cluster, per the frozen-faces contract), then delete the
        // person. The faces go back to the unassigned pool, and the recluster
        // watermark resets so the next gated pass regroups them: without the
        // reset, these pre-existing face ids sit below the watermark and the
        // gate stays closed forever.
        let conn = seed();
        assign(&conn, &[1, 2], "Alice").unwrap();
        assert_eq!(faces_list(&conn).unwrap().people.len(), 1);
        // Simulate a completed recluster covering these faces: the watermark
        // sits at their ids, so the gate would stay closed for them forever.
        videre_core::face_db::advance_recluster_watermark(&conn).unwrap();
        assert!(videre_core::face_db::recluster_watermark(&conn).unwrap() > 0);

        delete_person(&conn, "Alice").unwrap();
        assert_eq!(faces_list(&conn).unwrap().people.len(), 0, "Alice is gone");
        assert_eq!(
            videre_core::face_db::recluster_watermark(&conn).unwrap(),
            0,
            "deleting a person must reopen the gated regroup for their faces"
        );
        let rows: Vec<(Option<i64>, Option<String>, i64)> = {
            let mut s = conn
                .prepare("SELECT cluster_id, person_label, confirmed FROM faces WHERE id IN (1, 2) ORDER BY id")
                .unwrap();
            s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert!(
            rows.iter()
                .all(|(cid, label, confirmed)| cid.is_none() && label.is_none() && *confirmed == 0),
            "every face returns to the unassigned pool: {rows:?}"
        );
    }

    #[test]
    fn set_primary_is_exclusive_per_person() {
        let conn = seed();
        set_primary(&conn, 2, "Alice").unwrap();
        let primaries: Vec<i64> = {
            let mut s = conn
                .prepare("SELECT id FROM faces WHERE person_label='alice' AND is_primary=1")
                .unwrap();
            s.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(primaries, vec![2], "exactly one primary, now face 2");
    }

    #[test]
    fn renaming_only_the_spelling_keeps_the_identity() {
        // The common rename: correcting or extending what is shown, which must
        // not change the URL or touch a single face row.
        let conn = seed();
        set_full_name(&conn, "alice", "Alice Smith").unwrap();
        let (name, full): (String, String) = conn
            .query_row("SELECT name, full_name FROM people", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(name, "alice", "identity is unchanged");
        assert_eq!(full, "Alice Smith", "only the display name moved");
        assert_eq!(person_detail(&conn, "alice").unwrap().faces.len(), 2);
    }

    // A write against a client-supplied id that matches no row is `Ok(0)` from
    // rusqlite, not an error. Reported as success it tells the labeling UI an
    // action worked when nothing changed. Each handler that takes an id from the
    // client must turn "matched nothing" into NotFound, the way set_full_name
    // already does.

    #[test]
    fn assign_a_missing_face_is_not_found() {
        let conn = seed();
        assert!(matches!(assign(&conn, &[999], "Bob"), Err(Error::NotFound)));
    }

    #[test]
    fn assign_is_atomic_when_one_face_is_missing() {
        // face 3 exists, 999 does not. All-or-nothing: face 3 must be untouched
        // and no `Bob` person may be created, so a partial write can never be
        // reported as success.
        let conn = seed();
        assert!(matches!(
            assign(&conn, &[3, 999], "Bob"),
            Err(Error::NotFound)
        ));
        let (label, confirmed): (Option<String>, i64) = conn
            .query_row(
                "SELECT person_label, confirmed FROM faces WHERE id = 3",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(label, None, "face 3 must not have been labelled");
        assert_eq!(confirmed, 0, "face 3 must not have been confirmed");
        let bob: i64 = conn
            .query_row("SELECT COUNT(*) FROM people WHERE name = 'bob'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(
            bob, 0,
            "no person may be created when the assign rolls back"
        );
    }

    #[test]
    fn assign_commit_failure_rolls_back_and_closes_the_transaction() {
        let conn = seed();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE commit_guard_parent (id INTEGER PRIMARY KEY);
             CREATE TABLE commit_guard_child (
                 face_id INTEGER PRIMARY KEY,
                 parent_id INTEGER NOT NULL,
                 FOREIGN KEY(parent_id) REFERENCES commit_guard_parent(id)
                     DEFERRABLE INITIALLY DEFERRED
             );
             CREATE TRIGGER fail_assign_commit
             AFTER UPDATE ON faces
             WHEN NEW.id = 3
             BEGIN
                 INSERT INTO commit_guard_child (face_id, parent_id)
                 VALUES (NEW.id, 999);
             END;",
        )
        .unwrap();

        assert!(assign(&conn, &[3], "Bob").is_err());
        assert!(conn.is_autocommit());
        let state: (Option<String>, i64) = conn
            .query_row(
                "SELECT person_label, confirmed FROM faces WHERE id = 3",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, (None, 0));
        let bob: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE name = 'bob'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bob, 0);
    }

    #[test]
    fn assign_rejects_empty_face_ids() {
        // Nothing to assign is a malformed request, not a silent success that
        // creates a person with no faces.
        let conn = seed();
        assert!(matches!(assign(&conn, &[], "Bob"), Err(Error::Invalid)));
    }

    #[test]
    fn remove_face_missing_is_not_found() {
        let conn = seed();
        assert!(matches!(remove_face(&conn, 999), Err(Error::NotFound)));
    }

    #[test]
    fn dissolve_cluster_missing_is_not_found() {
        let conn = seed();
        assert!(matches!(dissolve_cluster(&conn, 999), Err(Error::NotFound)));
    }

    #[test]
    fn set_primary_missing_face_is_not_found() {
        let conn = seed();
        assert!(matches!(
            set_primary(&conn, 999, "Alice"),
            Err(Error::NotFound)
        ));
    }

    #[test]
    fn set_primary_face_of_another_person_is_not_found_and_rolls_back() {
        // face 5 is an unassigned singleton, so the guarded update matches no
        // row for Alice. The failure must roll back the primary-clearing step:
        // Alice's existing primary (face 1) has to survive.
        let conn = seed();
        assert!(matches!(
            set_primary(&conn, 5, "Alice"),
            Err(Error::NotFound)
        ));
        let primary: i64 = conn
            .query_row(
                "SELECT id FROM faces WHERE person_label = 'alice' AND is_primary = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            primary, 1,
            "the original primary must be restored on rollback"
        );
    }

    #[test]
    fn delete_person_missing_is_idempotent_success() {
        // Delete is idempotent: asking to unassign a person who is already gone
        // has already achieved its goal. A person can also legitimately have a
        // `people` row and no confirmed faces, which would make a row-count
        // check wrongly 404 a real person, so delete stays out of the NotFound
        // rule by design.
        let conn = seed();
        assert!(delete_person(&conn, "Nobody").is_ok());
    }
}

#[cfg(test)]
mod identity_tests {
    use super::tests::seed;
    use super::*;

    fn people(conn: &Connection) -> Vec<(String, String)> {
        conn.prepare("SELECT name, full_name FROM people ORDER BY name")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn assign_stores_the_identity_and_records_the_display_name() {
        let conn = seed();
        assign(&conn, &[3], "Işıl Özyeğin").unwrap();

        let label: String = conn
            .query_row("SELECT person_label FROM faces WHERE id = 3", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(label, "isil_ozyegin", "faces hold the identity");
        assert!(
            people(&conn).contains(&("isil_ozyegin".into(), "Işıl Özyeğin".into())),
            "and the spelling is kept for display"
        );
    }

    #[test]
    fn assigning_an_existing_name_in_another_case_joins_that_person() {
        // The bug this whole change exists to fix: this used to create a second
        // person.
        let conn = seed();
        assign(&conn, &[3], "ALICE").unwrap();
        assert_eq!(people(&conn).len(), 1, "still one person, not two");
        assert_eq!(person_detail(&conn, "alice").unwrap().faces.len(), 3);
        assert_eq!(
            people(&conn)[0].1,
            "Alice",
            "the existing spelling is not overwritten by the new casing"
        );
    }

    #[test]
    fn assign_rejects_a_name_with_no_usable_identity() {
        // Punctuation alone leaves nothing to identify a person by, and an
        // empty identity would be a person nobody could address.
        let conn = seed();
        assert!(matches!(assign(&conn, &[3], "!!!"), Err(Error::Invalid)));
    }

    #[test]
    fn person_detail_resolves_every_form_of_the_name() {
        let conn = seed();
        for form in ["alice", "Alice", "ALICE", "  alice  "] {
            assert_eq!(
                person_detail(&conn, form).unwrap().faces.len(),
                2,
                "form {form:?}"
            );
        }
    }

    #[test]
    fn person_detail_reports_the_display_name() {
        let d = person_detail(&seed(), "alice").unwrap();
        assert_eq!(d.label, "alice");
        assert_eq!(d.full_name, "Alice");
    }

    #[test]
    fn person_detail_falls_back_when_there_is_no_people_row() {
        // A label written before the table existed still has to render. The
        // orphan label is the pre-v2 shape, so its seed runs with enforcement
        // lifted and restored.
        let conn = seed();
        conn.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
        conn.execute(
            "INSERT INTO faces (id,hash,bbox,embedding,person_label,confirmed) \
             VALUES (9,'h9','0,0,9,9',X'0000','orphan',1)",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON").unwrap();
        let d = person_detail(&conn, "orphan").unwrap();
        assert_eq!(d.full_name, "orphan", "falls back to the identity");
    }

    #[test]
    fn set_full_name_changes_only_the_display_name() {
        let conn = seed();
        set_full_name(&conn, "alice", "Alice Smith").unwrap();
        assert_eq!(people(&conn), vec![("alice".into(), "Alice Smith".into())]);
        assert_eq!(
            person_detail(&conn, "alice").unwrap().faces.len(),
            2,
            "no face was touched"
        );
    }

    #[test]
    fn set_full_name_accepts_any_form_of_the_identity() {
        let conn = seed();
        set_full_name(&conn, "ALICE", "Alice Smith").unwrap();
        assert_eq!(people(&conn)[0].1, "Alice Smith");
    }

    #[test]
    fn set_full_name_on_a_missing_person_is_not_found() {
        assert!(matches!(
            set_full_name(&seed(), "nobody", "Someone"),
            Err(Error::NotFound)
        ));
    }

    #[test]
    fn set_full_name_rejects_an_empty_display_name() {
        // A person with no name to show is worse than one shown by identity.
        assert!(matches!(
            set_full_name(&seed(), "alice", "   "),
            Err(Error::Invalid)
        ));
    }

    #[test]
    fn delete_person_accepts_any_form_of_the_name() {
        let conn = seed();
        delete_person(&conn, "Alice").unwrap();
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM faces WHERE person_label IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "faces are unassigned whichever form was passed");
    }

    #[test]
    fn set_primary_accepts_any_form_of_the_name() {
        let conn = seed();
        set_primary(&conn, 2, "ALICE").unwrap();
        let primary: i64 = conn
            .query_row(
                "SELECT id FROM faces WHERE person_label='alice' AND is_primary=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(primary, 2);
    }
}

#[cfg(test)]
mod never_run_tests {
    use super::*;

    /// :warning: **A scanned-but-never-detected library has no `faces` table.**
    ///
    /// `videre scan` creates `file_hashes`, `people` and `pipeline_runs`. The
    /// faces table arrives with the first `videre faces` run, so every query
    /// here failed with "no such table" until then. The server turned that into
    /// a 500 with an empty body, and the page turned the empty body into
    /// `Unexpected end of JSON input` across the top of the labeling UI.
    ///
    /// Every existing test in this file seeds a faces table, which is why none
    /// of them could see it: they all describe a library that has already run
    /// detection.
    #[test]
    fn a_library_that_never_ran_detection_is_empty_not_an_error() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE file_hashes (path TEXT PRIMARY KEY, hash TEXT NOT NULL);
             CREATE TABLE people (name TEXT PRIMARY KEY, full_name TEXT);",
        )
        .unwrap();

        let data = faces_list(&conn).expect("a library with no faces table is not an error");
        assert!(data.people.is_empty());
        assert!(data.clusters.is_empty());
        assert!(data.singletons.is_empty());
    }

    /// A real initialized library: foreign keys verified on, the canonical DDL
    /// in place. The public face paths must work unchanged and orphan writes
    /// must fail.
    #[test]
    fn an_initialized_library_enforces_keys_through_the_public_paths() {
        let root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let ctx = videre_core::library::LibraryContext::new(root.path(), cache.path()).unwrap();
        let conn = videre_core::library_db::initialize(&ctx).unwrap();
        let keys_on: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(keys_on, 1, "an initialized library verifies enforcement");

        conn.execute_batch(
            "INSERT INTO faces (id, hash, bbox, embedding, cluster_id, confirmed, det_score, blur) VALUES
                (1, 'k1', '0,0,9,9', X'0000', 7, 0, 0.9, 600.0),
                (2, 'k2', '0,0,9,9', X'0000', 7, 0, 0.9, 600.0);
             INSERT INTO people (name, full_name) VALUES ('ayse', 'Ayşe'), ('bulent', 'Bülent');",
        )
        .unwrap();
        assign(&conn, &[1], "Ayşe").unwrap();
        assign(&conn, &[2], "Bülent").unwrap();

        // An orphan write fails: a face labeled with an unknown person.
        assert!(conn
            .execute(
                "INSERT INTO faces (hash,bbox,embedding,person_label,confirmed)
                 VALUES ('k9','0,0,9,9',X'0000','hayalet',1)",
                [],
            )
            .is_err());

        // Deleting a person returns the face to the unassigned pool.
        delete_person(&conn, "Ayşe").unwrap();
        let state: (i64, Option<String>) = conn
            .query_row(
                "SELECT confirmed, person_label FROM faces WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(state, (0, None));

        videre_core::face_db::reset_all(&conn).unwrap();
        let violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(violations, 0);
    }
}
