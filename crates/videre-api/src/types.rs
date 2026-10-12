use serde::Serialize;

/// One labeled person: their confirmed faces plus a representative face id
/// (the primary, or lowest id) used as the card thumbnail.
#[derive(Serialize, Clone)]
pub struct PersonData {
    /// Identity: normalized, what URLs and actions use.
    pub label: String,
    /// What a reader sees. Falls back to `label` for a person whose row
    /// predates the people table.
    pub full_name: String,
    pub face_ids: Vec<i64>,
    pub representative_id: i64,
    pub hashes: Vec<String>,
}

/// One unassigned cluster (green section in the labeling UI).
#[derive(Serialize, Clone)]
pub struct ClusterData {
    pub cluster_id: i64,
    pub face_ids: Vec<i64>,
    pub hashes: Vec<String>,
}

/// One unclustered, unassigned face (orange section).
#[derive(Serialize, Clone)]
pub struct SingletonData {
    pub face_id: i64,
    pub hash: String,
}

/// Top-level payload for the labeling page.
///
/// `Default` is the "detection has never run" answer, which is a state rather
/// than an error. See `faces_list`.
#[derive(Serialize, Clone, Default)]
pub struct FacesData {
    pub people: Vec<PersonData>,
    pub clusters: Vec<ClusterData>,
    /// One page of the singles; see `faces_list_page`.
    pub singletons: Vec<SingletonData>,
    /// Every single the request's filter keeps, not just this page.
    pub singles_total: usize,
    /// The face id to page after for the next page, `None` at the end.
    pub singles_next: Option<i64>,
}

/// A page of faces in keyset order: those after face `after` (from the
/// start when `None`), at most `limit` of them. Keyset, not an offset, so a
/// page stays where it was when faces before it are assigned away.
#[derive(Clone, Copy, Debug)]
pub struct Page {
    pub after: Option<i64>,
    pub limit: usize,
}

impl Page {
    pub const ALL: Page = Page {
        after: None,
        limit: usize::MAX,
    };
    pub fn first(limit: usize) -> Page {
        Page { after: None, limit }
    }
    pub fn after(after: i64, limit: usize) -> Page {
        Page {
            after: Some(after),
            limit,
        }
    }
}

/// One face row on a cluster detail page.
#[derive(Serialize, Clone)]
pub struct ClusterFaceData {
    pub face_id: i64,
    pub hash: String,
    pub path: String,
}

/// Cluster detail: every face in one unassigned cluster.
#[derive(Serialize, Clone)]
pub struct ClusterDetail {
    pub cluster_id: i64,
    pub faces: Vec<ClusterFaceData>,
}

/// One face row on a person detail page. `is_primary` marks the current
/// default photo (the person's thumbnail on the labeling page).
#[derive(Serialize, Clone)]
pub struct PersonFaceData {
    pub face_id: i64,
    pub hash: String,
    pub path: String,
    pub is_primary: bool,
}

/// Person detail: every confirmed face for one person.
#[derive(Serialize, Clone)]
pub struct PersonDetail {
    /// Identity: what the URL and every face row use.
    pub label: String,
    /// What a reader sees, and what the page lets them edit.
    pub full_name: String,
    /// One page of the faces; see `person_detail_page`.
    pub faces: Vec<PersonFaceData>,
    /// Every confirmed face of this person, not just this page.
    pub face_total: usize,
    /// The face id to page after for the next page, `None` at the end.
    pub next: Option<i64>,
}
