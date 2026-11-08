#[derive(Clone, Debug, PartialEq)]
pub struct Folder {
    pub id: i64,
    pub path: String,
    pub is_enabled: bool,
    pub last_scanned: Option<String>,
    pub added_at: String,
}
