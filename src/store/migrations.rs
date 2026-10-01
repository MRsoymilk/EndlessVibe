use anyhow::{bail,Result};
use rusqlite::Connection;

pub const CURRENT_SCHEMA_VERSION:i64=2;

const CREATE_SCHEMA_V1:&str=r#"
CREATE TABLE IF NOT EXISTS kv(
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    expires INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(namespace,key)
);
CREATE TABLE IF NOT EXISTS jobs(
    id TEXT PRIMARY KEY,
    data TEXT NOT NULL,
    output BLOB NOT NULL DEFAULT X'',
    output_offset INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS audit(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    time INTEGER NOT NULL,
    tool TEXT NOT NULL,
    workspace TEXT NOT NULL,
    outcome TEXT NOT NULL,
    note TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS traffic(
    bucket INTEGER PRIMARY KEY,
    requests INTEGER NOT NULL DEFAULT 0,
    rx_bytes INTEGER NOT NULL DEFAULT 0,
    tx_bytes INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS operation_log(
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    started INTEGER NOT NULL,
    finished INTEGER,
    tool TEXT NOT NULL,
    workspace TEXT NOT NULL,
    project TEXT NOT NULL,
    status TEXT NOT NULL,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    input_json TEXT NOT NULL,
    output_json TEXT NOT NULL DEFAULT '{}',
    diff TEXT NOT NULL DEFAULT '',
    added_lines INTEGER NOT NULL DEFAULT 0,
    removed_lines INTEGER NOT NULL DEFAULT 0,
    error TEXT NOT NULL DEFAULT ''
);
"#;

const CREATE_INDEXES_V2:&str=r#"
CREATE INDEX IF NOT EXISTS idx_kv_expiry
ON kv(expires) WHERE expires>0;

CREATE INDEX IF NOT EXISTS idx_task_checkpoint_updated
ON kv(CAST(json_extract(value,'$.updated') AS INTEGER) DESC)
WHERE namespace='task_checkpoints';

CREATE INDEX IF NOT EXISTS idx_task_checkpoint_lookup
ON kv(
    json_extract(value,'$.workspace'),
    json_extract(value,'$.project'),
    json_extract(value,'$.task_id'),
    CAST(json_extract(value,'$.updated') AS INTEGER) DESC
)
WHERE namespace='task_checkpoints';

CREATE INDEX IF NOT EXISTS idx_jobs_created
ON jobs(CAST(json_extract(data,'$.created') AS INTEGER) DESC);

CREATE INDEX IF NOT EXISTS idx_jobs_status_created
ON jobs(
    json_extract(data,'$.status'),
    CAST(json_extract(data,'$.created') AS INTEGER) DESC
);

CREATE INDEX IF NOT EXISTS idx_jobs_target_created
ON jobs(
    json_extract(data,'$.workspace'),
    json_extract(data,'$.project'),
    json_extract(data,'$.task_id'),
    CAST(json_extract(data,'$.created') AS INTEGER) DESC
);

CREATE INDEX IF NOT EXISTS idx_audit_time
ON audit(time);

CREATE INDEX IF NOT EXISTS idx_audit_tool_time
ON audit(tool,time);

CREATE INDEX IF NOT EXISTS idx_operation_started
ON operation_log(started);

CREATE INDEX IF NOT EXISTS idx_operation_status_started
ON operation_log(status,started);

CREATE INDEX IF NOT EXISTS idx_operation_target_started
ON operation_log(workspace,project,started);
"#;

pub fn migrate(connection:&mut Connection)->Result<()>{
    let version:i64=connection.query_row("PRAGMA user_version",[],|r|r.get(0))?;
    if version>CURRENT_SCHEMA_VERSION{
        bail!("Database belongs to a newer schema; refusing to downgrade");
    }
    if version==CURRENT_SCHEMA_VERSION{return Ok(());}

    let tx=connection.transaction()?;
    if version<1{
        tx.execute_batch(CREATE_SCHEMA_V1)?;
    }
    if version<2{
        tx.execute_batch(CREATE_INDEXES_V2)?;
    }
    tx.execute_batch(&format!("PRAGMA user_version={CURRENT_SCHEMA_VERSION};"))?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn fresh_database_reaches_current_schema(){
        let d=tempfile::tempdir().unwrap();
        let path=d.path().join("db");
        let mut c=Connection::open(path).unwrap();
        migrate(&mut c).unwrap();
        let version:i64=c.query_row("PRAGMA user_version",[],|r|r.get(0)).unwrap();
        assert_eq!(version,CURRENT_SCHEMA_VERSION);
        let count:i64=c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_operation_status_started'",[],|r|r.get(0)).unwrap();
        assert_eq!(count,1);
    }
}
