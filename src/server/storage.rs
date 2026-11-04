//! Persistent review cache and signed-event outbox. SQLite work stays off the executor.
use super::{
    error::AppError,
    nostr::events::{self, Decoded},
};
use crate::types::*;
use chrono::{DateTime, Utc};
use nostr_sdk::prelude::*;
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS events (
    event_id TEXT PRIMARY KEY, kind INTEGER NOT NULL, pubkey TEXT NOT NULL,
    d TEXT NOT NULL, created_at INTEGER NOT NULL, content TEXT NOT NULL,
    rating INTEGER CHECK(rating BETWEEN 0 AND 5), event_json TEXT NOT NULL,
    published INTEGER NOT NULL DEFAULT 0, UNIQUE(kind, pubkey, d)
);
CREATE TABLE IF NOT EXISTS targets (
    event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
    url TEXT NOT NULL, identifier TEXT NOT NULL, PRIMARY KEY(event_id, url, identifier)
);
CREATE INDEX IF NOT EXISTS targets_url ON targets(url);
CREATE INDEX IF NOT EXISTS targets_identifier ON targets(identifier);
CREATE TABLE IF NOT EXISTS outbox (
    event_id TEXT PRIMARY KEY REFERENCES events(event_id) ON DELETE CASCADE,
    queued_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS ingest_progress (
    feed TEXT PRIMARY KEY, completed_until INTEGER NOT NULL DEFAULT 0,
    backfill_until INTEGER, backfill_start INTEGER, last_full_scan INTEGER NOT NULL DEFAULT 0
);
CREATE VIEW IF NOT EXISTS mint_urls AS
    SELECT t.url, MAX(e.created_at) AS updated_at FROM targets t
    JOIN events e USING(event_id) WHERE t.url != '' GROUP BY t.url;
CREATE VIEW IF NOT EXISTS announcements AS
    SELECT t.url, e.* FROM targets t JOIN events e USING(event_id)
    WHERE e.kind = 38172 AND t.url != '';
CREATE VIEW IF NOT EXISTS current_reviews AS
    SELECT * FROM (
        SELECT e.event_id, e.pubkey, e.content, e.rating, e.created_at, e.published, u.url AS mint_url,
            ROW_NUMBER() OVER(PARTITION BY u.url, e.pubkey ORDER BY e.created_at DESC, e.event_id ASC) AS rank
        FROM events e JOIN targets t USING(event_id) JOIN mint_urls u
        ON t.url = u.url OR (t.identifier != '' AND EXISTS (
            SELECT 1 FROM announcements a WHERE a.url = u.url
            AND (a.d = t.identifier OR '38172:' || a.pubkey || ':' || a.d = t.identifier)
        )) WHERE e.kind = 38000
        GROUP BY u.url, e.event_id
    ) WHERE rank = 1 AND (rating IS NOT NULL OR TRIM(content) != '');
CREATE VIEW IF NOT EXISTS directory AS
    SELECT u.url, COALESCE((SELECT a.created_at FROM announcements a WHERE a.url=u.url
        ORDER BY (a.pubkey=a.d) DESC,a.created_at DESC,a.event_id ASC LIMIT 1),u.updated_at) AS updated_at,
        (SELECT a.content FROM announcements a WHERE a.url = u.url
         ORDER BY (a.pubkey = a.d) DESC, a.created_at DESC, a.event_id ASC LIMIT 1) AS info,
        COALESCE(s.total, 0) AS review_count, s.avg AS review_avg
    FROM mint_urls u LEFT JOIN (
        SELECT mint_url, COUNT(*) AS total, AVG(rating) AS avg FROM current_reviews GROUP BY mint_url
    ) s ON s.mint_url = u.url;
"#;

pub struct Database {
    conn: Arc<Mutex<Connection>>,
}
impl Database {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        Self::init(Connection::open(path)?)
    }
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, AppError> {
        Self::init(Connection::open_in_memory()?)
    }
    fn init(conn: Connection) -> Result<Self, AppError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
    pub async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, AppError> + Send + 'static,
    ) -> Result<T, AppError> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| AppError::Internal("database lock poisoned".into()))?;
            f(&guard)
        })
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
    }
    pub async fn check(&self) -> Result<(), AppError> {
        self.run(|c| {
            c.query_row("SELECT COUNT(*) FROM ingest_progress", [], |_| Ok(()))?;
            Ok(())
        })
        .await
    }
    pub async fn has_data(&self) -> Result<bool, AppError> {
        self.run(|c| Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM events)", [], |r| r.get(0))?))
            .await
    }
    pub async fn store(
        &self,
        event: &Event,
        decoded: Decoded,
        enqueue: bool,
    ) -> Result<bool, AppError> {
        let event = event.clone();
        self.run(move |c| {
            let tx = c.unchecked_transaction()?;
            let old: Option<(String, u64)> = tx.query_row(
                "SELECT event_id, created_at FROM events WHERE kind=?1 AND pubkey=?2 AND d=?3",
                params![event.kind.as_u16(), event.pubkey.to_hex(), decoded.d], |r| Ok((r.get(0)?, r.get(1)?))
            ).optional()?;
            let id = event.id.to_hex();
            if let Some((old_id, at)) = &old {
                if old_id != &id && (*at > event.created_at.as_secs() || (*at == event.created_at.as_secs() && old_id < &id)) { return Ok(false); }
                if old_id != &id { tx.execute("DELETE FROM events WHERE event_id=?1", [old_id])?; }
            }
            tx.execute("INSERT OR IGNORE INTO events(event_id,kind,pubkey,d,created_at,content,rating,event_json,published) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![id, event.kind.as_u16(), event.pubkey.to_hex(), decoded.d, event.created_at.as_secs(), decoded.content, decoded.rating, event.as_json(), !enqueue])?;
            for target in decoded.targets {
                tx.execute("INSERT OR IGNORE INTO targets(event_id,url,identifier) VALUES(?1,?2,?3)", params![id, target.url, target.identifier])?;
            }
            if enqueue {
                tx.execute("INSERT OR IGNORE INTO outbox(event_id,queued_at) SELECT event_id,?2 FROM events WHERE event_id=?1 AND published=0", params![id, Utc::now().timestamp()])?;
            } else {
                tx.execute("UPDATE events SET published=1 WHERE event_id=?1", [&id])?;
                tx.execute("DELETE FROM outbox WHERE event_id=?1", [&id])?;
            }
            tx.commit()?;
            Ok(true)
        }).await
    }
    pub async fn pending(&self) -> Result<Vec<Event>, AppError> {
        self.run(|c| {
            let mut s = c.prepare("SELECT e.event_json FROM outbox o JOIN events e USING(event_id) WHERE queued_at <= unixepoch() ORDER BY queued_at,event_id LIMIT 20")?;
            let raw = s.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
            raw.into_iter().map(|s| Event::from_json(s).map_err(|e| AppError::Internal(e.to_string()))).collect()
        }).await
    }
    pub async fn retry_later(&self, id: String) -> Result<(), AppError> {
        self.run(move |c| {
            c.execute(
                "UPDATE outbox SET queued_at=unixepoch()+30 WHERE event_id=?1",
                [id],
            )?;
            Ok(())
        })
        .await
    }
    pub async fn mark_published(&self, id: String) -> Result<(), AppError> {
        self.run(move |c| {
            let tx = c.unchecked_transaction()?;
            tx.execute("UPDATE events SET published=1 WHERE event_id=?1", [&id])?;
            tx.execute("DELETE FROM outbox WHERE event_id=?1", [&id])?;
            tx.commit()?;
            Ok(())
        })
        .await
    }
    pub async fn published(&self, id: String) -> Result<Option<bool>, AppError> {
        self.run(move |c| {
            Ok(c.query_row(
                "SELECT published FROM events WHERE event_id=?1 AND kind=38000",
                [id],
                |r| r.get(0),
            )
            .optional()?)
        })
        .await
    }
    pub async fn search_mints(
        &self,
        search: String,
        sort: MintSort,
        page: u64,
    ) -> Result<MintPage, AppError> {
        self.run(move |c| {
            let filter = " WHERE instr(lower(url),lower(?1))>0 OR instr(lower(CASE WHEN json_valid(info) THEN COALESCE(json_extract(info,'$.name'),'') ELSE '' END),lower(?1))>0";
            let total = c.query_row(&format!("SELECT COUNT(*) FROM directory{filter}"), [&search], |r| r.get(0))?;
            let order = match sort {
                MintSort::Name => "lower(CASE WHEN json_valid(info) THEN COALESCE(json_extract(info,'$.name'),url) ELSE url END),url",
                MintSort::RatingHigh => "review_avg IS NULL,review_avg DESC,review_count DESC,url",
                MintSort::RatingLow => "review_avg IS NULL,review_avg ASC,review_count DESC,url",
                MintSort::MostReviews => "review_count DESC,review_avg DESC,url",
            };
            let mut s = c.prepare(&format!("SELECT url,info,updated_at,review_count,review_avg FROM directory{filter} ORDER BY {order} LIMIT 20 OFFSET ?2"))?;
            let mints = s.query_map(params![search, page.saturating_mul(20).min(i64::MAX as u64)], read_mint)?.collect::<Result<_, _>>()?;
            Ok(MintPage { mints, total })
        }).await
    }
    pub async fn mint(&self, url: String) -> Result<Option<MintRead>, AppError> {
        self.run(move |c| {
            Ok(c.query_row(
                "SELECT url,info,updated_at,review_count,review_avg FROM directory WHERE url=?1",
                [url],
                read_mint,
            )
            .optional()?)
        })
        .await
    }
    pub async fn reviews(
        &self,
        url: String,
        limit: u64,
        offset: u64,
    ) -> Result<MintReviews, AppError> {
        self.run(move |c| {
            let (total, rating_count, rating_avg) = c.query_row("SELECT COUNT(*),COUNT(rating),AVG(rating) FROM current_reviews WHERE mint_url=?1", [&url], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            let mut s = c.prepare("SELECT event_id,pubkey,rating,content,created_at,published FROM current_reviews WHERE mint_url=?1 ORDER BY created_at DESC,event_id ASC LIMIT ?2 OFFSET ?3")?;
            let reviews = s.query_map(params![url, limit.clamp(1,500), offset.min(i64::MAX as u64)], |r| {
                let pubkey: String = r.get(1)?;
                let npub = PublicKey::from_hex(&pubkey).ok().and_then(|p| p.to_bech32().ok()).unwrap_or_default();
                Ok(ReviewRead { event_id: r.get(0)?, pubkey, npub, rating: r.get(2)?, content: r.get(3)?, created_at: datetime(r.get(4)?), published: r.get(5)? })
            })?.collect::<Result<_, _>>()?;
            let announcement: Option<(String,String)> = c.query_row("SELECT d,pubkey FROM announcements WHERE url=?1 ORDER BY (pubkey=d) DESC,created_at DESC,event_id ASC LIMIT 1", [&url], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
            let (mint_pubkey, announcer) = announcement.filter(|(d,_)| events::mint_pubkey(d)).map(|(d,p)| (Some(d),Some(p))).unwrap_or_default();
            Ok(MintReviews { reviews,total,rating_count,rating_avg,context: ReviewContext { mint_url: url,mint_pubkey,announcer } })
        }).await
    }
}

fn datetime(seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(seconds, 0).unwrap_or_default()
}
fn read_mint(r: &rusqlite::Row<'_>) -> rusqlite::Result<MintRead> {
    let url: String = r.get(0)?;
    let raw: Option<String> = r.get(1)?;
    let mut info: serde_json::Value = raw
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    // NIP-87 announcements use kind-0 metadata; the existing UI uses NUT-06 names.
    if let Some(object) = info.as_object_mut() {
        if !object.contains_key("description") {
            if let Some(about) = object.get("about").cloned() {
                object.insert("description".into(), about);
            }
        }
        if !object.contains_key("icon_url") {
            if let Some(picture) = object.get("picture").cloned() {
                object.insert("icon_url".into(), picture);
            }
        }
    }
    let name = info
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    Ok(MintRead {
        id: mint_key(&url),
        url,
        name,
        info: info.is_object().then(|| info.to_string()),
        updated_at: datetime(r.get(2)?),
        review_count: r.get(3)?,
        review_avg: r.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use events::tests::event;
    async fn store(db: &Database, event: &Event) {
        db.store(event, events::decode(event).unwrap(), false)
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn joins_legacy_current_and_address_only_reviews_once_per_author() {
        let db = Database::open_in_memory().unwrap();
        let operator = Keys::generate();
        let author = Keys::generate();
        let pk = operator.public_key().to_hex();
        let url = "https://mint.example";
        // Reviews may arrive before their announcement and use a URL as d.
        let legacy = event(&author, 38000, &[&["d", url]], "[2/5] Old", 100);
        store(&db, &legacy).await;
        store(&db, &legacy).await;
        store(
            &db,
            &event(
                &operator,
                38172,
                &[&["d", &pk], &["u", url]],
                r#"{"name":"Copper","about":"A mint","picture":"https://mint.example/icon.png"}"#,
                90,
            ),
        )
        .await;
        let current = event(
            &author,
            38000,
            &[
                &["k", "38172"],
                &["d", &pk],
                &["u", "https://MINT.example:443/"],
            ],
            "[5/5] New",
            200,
        );
        store(&db, &current).await;
        let other = Keys::generate();
        let address = format!("38172:{pk}:{pk}");
        store(
            &db,
            &event(&other, 38000, &[&["a", &address]], "No star rating", 250),
        )
        .await;
        let data = db.reviews(url.into(), 100, 0).await.unwrap();
        assert_eq!(
            (data.total, data.rating_count, data.rating_avg),
            (2, 1, Some(5.0))
        );
        assert_eq!(data.reviews[0].content, "No star rating");
        assert_eq!(data.context.mint_pubkey, Some(pk));
        let listing = db
            .search_mints("Copper".into(), MintSort::RatingHigh, 0)
            .await
            .unwrap();
        assert_eq!(listing.total, 1);
        assert_eq!(listing.mints[0].review_count, data.total);
        assert_eq!(listing.mints[0].review_avg, data.rating_avg);
        assert!(listing.mints[0].info.as_ref().unwrap().contains("icon_url"));
        assert_eq!(db.reviews(url.into(), 1, 1).await.unwrap().reviews.len(), 1);
        // An older relay delivery cannot roll back the newer rating.
        store(
            &db,
            &event(
                &author,
                38000,
                &[
                    &["k", "38172"],
                    &["d", &data.context.mint_pubkey.unwrap()],
                    &["u", url],
                ],
                "[1/5] Stale",
                150,
            ),
        )
        .await;
        assert_eq!(
            db.mint(url.into()).await.unwrap().unwrap().review_avg,
            Some(5.0)
        );
    }
    #[tokio::test]
    async fn replacement_ties_and_multi_mint_recommendations_are_deterministic() {
        let keys = Keys::generate();
        let a = event(&keys, 38000, &[&["d", "https://a.example"]], "[1/5] A", 100);
        let b = event(&keys, 38000, &[&["d", "https://a.example"]], "[5/5] B", 100);
        let expected = if a.id < b.id { &a } else { &b };
        for order in [[&a, &b], [&b, &a]] {
            let db = Database::open_in_memory().unwrap();
            for e in order {
                store(&db, e).await;
            }
            let data = db.reviews("https://a.example".into(), 10, 0).await.unwrap();
            assert_eq!(data.total, 1);
            assert_eq!(data.reviews[0].event_id, expected.id.to_hex());
        }
        let db = Database::open_in_memory().unwrap();
        store(
            &db,
            &event(
                &keys,
                38000,
                &[
                    &["u", "https://a.example", "cashu"],
                    &["u", "https://b.example", "cashu"],
                ],
                "[3/5] Both",
                200,
            ),
        )
        .await;
        assert_eq!(
            db.search_mints(String::new(), MintSort::Name, 0)
                .await
                .unwrap()
                .total,
            2
        );
        for url in ["https://a.example", "https://b.example"] {
            assert_eq!(db.reviews(url.into(), 10, 0).await.unwrap().total, 1);
        }
    }
    #[tokio::test]
    async fn queued_reviews_survive_restart_and_delivery_acknowledgement() {
        let path = std::env::temp_dir().join(format!(
            "imf-test-{}.sqlite3",
            Keys::generate().public_key()
        ));
        let keys = Keys::generate();
        let review = event(
            &keys,
            38000,
            &[&["d", "https://a.example"]],
            "[4/5] Persistent",
            100,
        );
        {
            let db = Database::open(&path).unwrap();
            db.store(&review, events::decode(&review).unwrap(), true)
                .await
                .unwrap();
            assert_eq!(db.pending().await.unwrap().len(), 1);
        }
        {
            let db = Database::open(&path).unwrap();
            assert_eq!(db.pending().await.unwrap()[0].id, review.id);
            assert_eq!(
                db.reviews("https://a.example".into(), 10, 0)
                    .await
                    .unwrap()
                    .total,
                1
            );
            db.mark_published(review.id.to_hex()).await.unwrap();
            db.store(&review, events::decode(&review).unwrap(), true)
                .await
                .unwrap();
            assert!(db.pending().await.unwrap().is_empty());
            assert_eq!(db.published(review.id.to_hex()).await.unwrap(), Some(true));
        }
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn failed_delivery_does_not_starve_later_reviews() {
        let db = Database::open_in_memory().unwrap();
        let keys = Keys::generate();
        for n in 0..21 {
            let review = event(
                &keys,
                38000,
                &[&["d", &format!("https://mint-{n}.example")]],
                "[4/5] Queued",
                100,
            );
            db.store(&review, events::decode(&review).unwrap(), true)
                .await
                .unwrap();
        }
        let first = db.pending().await.unwrap();
        assert_eq!(first.len(), 20);
        for review in &first {
            db.retry_later(review.id.to_hex()).await.unwrap();
            assert_eq!(db.published(review.id.to_hex()).await.unwrap(), Some(false));
        }
        let next = db.pending().await.unwrap();
        assert_eq!(next.len(), 1);
        assert!(!first.iter().any(|e| e.id == next[0].id));
        // Advance the delivery schedule without a wall-clock sleep.
        db.run(|c| {
            c.execute("UPDATE outbox SET queued_at=0", [])?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(db.pending().await.unwrap().len(), 20);
    }
}
