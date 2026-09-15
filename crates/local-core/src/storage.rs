use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;

#[derive(Clone, Serialize)]
pub struct Message { pub id: String, pub peer_id: String, pub direction: String, pub channel: String, pub text: String, pub timestamp: i64 }

#[derive(Clone, Serialize)]
pub struct TrustedPeer { pub id: String, pub name: String }

pub struct Store(Connection);
impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        let conn = Connection::open(dir.join("local.sqlite3"))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS trusted (id TEXT PRIMARY KEY, name TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS messages (id TEXT NOT NULL, peer_id TEXT NOT NULL, direction TEXT NOT NULL, channel TEXT NOT NULL, text TEXT NOT NULL, timestamp INTEGER NOT NULL, PRIMARY KEY(id,peer_id,direction));
            CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS message_time ON messages(timestamp);")?;
        Ok(Self(conn))
    }
    pub fn trusted(&self, id: &str) -> bool { self.0.query_row("SELECT 1 FROM trusted WHERE id=?", [id], |_| Ok(())).is_ok() }
    pub fn trust(&self, id: &str, name: &str) -> Result<()> { self.0.execute("INSERT OR REPLACE INTO trusted VALUES (?,?)", params![id,name])?; Ok(()) }
    pub fn forget(&self, id: &str) -> Result<()> { self.0.execute("DELETE FROM trusted WHERE id=?", [id])?; Ok(()) }
    pub fn peers(&self) -> Result<Vec<TrustedPeer>> {
        let mut stmt = self.0.prepare("SELECT id,name FROM trusted ORDER BY name")?;
        let rows = stmt.query_map([], |r| Ok(TrustedPeer{id:r.get(0)?,name:r.get(1)?}))?;
        Ok(rows.collect::<std::result::Result<_,_>>()?)
    }
    pub fn insert_message(&self, m: &Message) -> Result<()> {
        self.0.execute("INSERT OR IGNORE INTO messages VALUES (?,?,?,?,?,?)", params![m.id,m.peer_id,m.direction,m.channel,m.text,m.timestamp])?;
        Ok(())
    }
    pub fn messages(&self) -> Result<Vec<Message>> {
        let mut stmt = self.0.prepare("SELECT id,peer_id,direction,channel,text,timestamp FROM (SELECT * FROM messages ORDER BY timestamp DESC,rowid DESC LIMIT 200) ORDER BY timestamp ASC")?;
        let rows = stmt.query_map([], |r| Ok(Message{id:r.get(0)?,peer_id:r.get(1)?,direction:r.get(2)?,channel:r.get(3)?,text:r.get(4)?,timestamp:r.get(5)?}))?;
        Ok(rows.collect::<std::result::Result<_,_>>()?)
    }
    pub fn name(&self) -> Option<String> { self.0.query_row("SELECT value FROM settings WHERE key='name'", [], |r| r.get(0)).ok() }
    pub fn set_name(&self, name: &str) -> Result<()> { self.0.execute("INSERT OR REPLACE INTO settings VALUES ('name',?)", [name])?; Ok(()) }
    pub fn clear_history(&self) -> Result<()> { self.0.execute("DELETE FROM messages", [])?; Ok(()) }
}

