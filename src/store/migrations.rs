use anyhow::{bail,Result};
use rusqlite::Connection;

pub const CURRENT_SCHEMA_VERSION:i64=4;

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

const ADD_ERROR_CODE_V3:&str=r#"
ALTER TABLE operation_log ADD COLUMN error_code TEXT NOT NULL DEFAULT '';
CREATE INDEX IF NOT EXISTS idx_operation_error_code_started
ON operation_log(error_code,started);
"#;

const CREATE_TRANSFER_HISTORY_INDEX_V4:&str=r#"
CREATE INDEX IF NOT EXISTS idx_transfer_history_scope_created
ON kv(
    json_extract(value,'$.peer_node_id'),
    json_extract(value,'$.workspace'),
    json_extract(value,'$.project'),
    CAST(json_extract(value,'$.created') AS INTEGER) DESC,
    key DESC
)
WHERE namespace='transfer_requests'
  AND json_extract(value,'$.request_id') IS NOT NULL
  AND json_extract(value,'$.request_id') != '';
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
    if version<3{
        tx.execute_batch(ADD_ERROR_CODE_V3)?;
    }
    if version<4{
        tx.execute_batch(CREATE_TRANSFER_HISTORY_INDEX_V4)?;
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
        let error_index:i64=c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_operation_error_code_started'",[],|r|r.get(0)).unwrap();
        assert_eq!(error_index,1);
        let transfer_index:i64=c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_transfer_history_scope_created'",[],|r|r.get(0)).unwrap();
        assert_eq!(transfer_index,1);
    }

    #[test]
    fn schema_v3_adds_scoped_transfer_history_index_without_losing_requests(){
        let mut c=Connection::open_in_memory().unwrap();
        c.execute_batch(CREATE_SCHEMA_V1).unwrap();
        c.execute_batch(CREATE_INDEXES_V2).unwrap();
        c.execute_batch(ADD_ERROR_CODE_V3).unwrap();
        let saved=serde_json::json!({"request_id":"req-01","peer_node_id":"parent",
            "workspace":"team","project":"project","created":1234,"updated":1234,
            "state":"completed","fingerprint":"safe","result":{"changed":true}});
        c.execute("INSERT INTO kv(namespace,key,value,expires) VALUES('transfer_requests','history-key',?1,0)",
            [saved.to_string()]).unwrap();
        c.execute_batch("PRAGMA user_version=3;").unwrap();
        migrate(&mut c).unwrap();
        assert_eq!(c.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),4);
        assert_eq!(c.query_row("SELECT value FROM kv WHERE namespace='transfer_requests' AND key='history-key'",
            [],|r|r.get::<_,String>(0)).unwrap(),saved.to_string());
        let sql="EXPLAIN QUERY PLAN SELECT key,value FROM kv WHERE namespace=?1
            AND json_extract(value,'$.peer_node_id')=?2
            AND json_extract(value,'$.workspace')=?3
            AND json_extract(value,'$.project')=?4
            AND json_extract(value,'$.request_id') IS NOT NULL
            AND json_extract(value,'$.request_id') != ''
            AND (?5 IS NULL OR CAST(json_extract(value,'$.created') AS INTEGER) < ?5
                 OR (CAST(json_extract(value,'$.created') AS INTEGER) = ?5 AND key < ?6))
            ORDER BY CAST(json_extract(value,'$.created') AS INTEGER) DESC,key DESC LIMIT ?7";
        let mut statement=c.prepare(sql).unwrap();
        for cursor in [None,Some(1234_i64)] {
            let plan=statement.query_map(rusqlite::params!["transfer_requests","parent","team","project",
                cursor,cursor.map(|_|"some-lower-key"),21_i64],
                |r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            assert!(plan.iter().any(|step|step.contains("idx_transfer_history_scope_created")),
                "Query plan did not use Transfer history index: {plan:?}");
        }
    }
    #[test]
    fn schema_v2_upgrades_to_v3_without_losing_data(){
        let mut c=Connection::open_in_memory().unwrap();
        c.execute_batch(CREATE_SCHEMA_V1).unwrap();
        c.execute_batch(CREATE_INDEXES_V2).unwrap();
        c.execute("INSERT INTO kv(namespace,key,value,expires) VALUES('x','keep','42',0)",[]).unwrap();
        c.execute_batch("PRAGMA user_version=2;").unwrap();
        migrate(&mut c).unwrap();
        assert_eq!(c.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),CURRENT_SCHEMA_VERSION);
        assert_eq!(c.query_row("SELECT value FROM kv WHERE namespace='x' AND key='keep'",[],|r|r.get::<_,String>(0)).unwrap(),"42");
        let columns:i64=c.query_row("SELECT COUNT(*) FROM pragma_table_info('operation_log') WHERE name='error_code'",[],|r|r.get(0)).unwrap();
        assert_eq!(columns,1);
    }
}
