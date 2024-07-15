use sqlx::FromRow;
use time::OffsetDateTime;

#[derive(FromRow, Debug, Clone)]
pub struct Feed {
    pub id: i64,
    pub name: String,
    pub last_updated: OffsetDateTime,
}

#[derive(FromRow, Debug, Clone)]
pub struct Entry {
    pub retrieved: OffsetDateTime,
    pub entry_id: String,
    pub title: String,
    pub description: String,
    pub url: String,
    pub author: Option<String>,
    pub published: Option<OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct FeedInfo {
    pub name: String,
    pub last_updated: OffsetDateTime,
    pub entry_count: usize,
}
