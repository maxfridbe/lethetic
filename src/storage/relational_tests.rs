use super::*;
use serde_json::{Value as Json, json};

fn native_session() -> Json {
    json!({"schema_version":5,"session_id":"test-session","display_name":"SQL session","system_prompt":"be useful", "theme_name":"dark",
 "messages":[
 {"role":"user","content":"calculate","tool_calls":null},
 {"role":"assistant","content":"","tool_calls":[{"id":"call-1","provider_id":"provider-1","function":{"name":"python","arguments":{"code":"print(1+1)"}}}],"provider_content":[{"type":"thinking","text":"calculate","signature":"opaque"}]},
 {"role":"tool","content":"2","tool_call_id":"call-1","tool_result_is_error":false},
 {"role":"assistant","content":"2","tool_calls":[]},
 {"role":"user","content":"next","tool_calls":null}],
 "blocks":[{"block_type":"Markdown","content":"**2**","success":true,"usage":{"output_tokens":3,"total_input_tokens":4,"breakdown_complete":true},"estimated_cost":null}],
 "history":["calculate","next"],"accounting":{"requests":[{"request_id":"r1","logical_turn_id":"turn-1","connection_id":"c1","model":"model-a","completed":true,"in_flight":false,"usage":{"uncached_input_tokens":4,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":3,"total_input_tokens":4,"breakdown_complete":true},"estimated_cost":{"currency":"USD","nanos":120,"incomplete":false}}],
 "latest_logical_turn_id":"turn-1","session":{"request_count":1,"usage":{"output_tokens":3}},"latest_logical_turn":{"request_count":1,"usage":{"output_tokens":3}}},
 "session_times":{"engine_ms":1,"tool_ms":2,"idle_ms":3},"tool_use_counts":{"python":3},"python_policy":{"custom":"unchanged"}})
}
fn read_json(db: &ContentDatabase, scope: &str, name: &str) -> Json {
    serde_json::from_slice(&db.read(scope, name).unwrap().unwrap()).unwrap()
}
fn count(db: &ContentDatabase, table: &str) -> i64 {
    db.query(&format!("SELECT COUNT(*) FROM {table}"), &[])
        .unwrap()[0][0]
        .as_int()
        .unwrap()
}

#[test]
fn normalized_rows_are_queryable_and_authoritative_on_reload() {
    let db = ContentDatabase::memory(&[]).unwrap();
    let session = native_session();
    db.put(
        "",
        "session_state.json",
        &serde_json::to_vec(&session).unwrap(),
    )
    .unwrap();
    assert_eq!(read_json(&db, "", "session_state.json"), session);
    assert_eq!(count(&db, "messages"), 5);
    assert_eq!(count(&db, "turns"), 2);
    assert_eq!(count(&db, "tool_calls"), 1);
    let result=db.query("SELECT m.role,t.function_name,json_extract(t.arguments_json,'$.code') FROM tool_calls t JOIN messages m ON m.id=t.message_id",&[]).unwrap();
    assert_eq!(result[0][0].to_text(), Some("assistant"));
    assert_eq!(result[0][1].to_text(), Some("python"));
    assert_eq!(result[0][2].to_text(), Some("print(1+1)"));
    assert_eq!(db.query("SELECT SUM(output_tokens),SUM(cost_nanos) FROM provider_requests WHERE model='model-a'",&[]).unwrap()[0][0].as_int(),Some(3));
    assert!(
        db.query(
            "SELECT name FROM sqlite_schema WHERE name='content_records'",
            &[]
        )
        .unwrap()
        .is_empty()
    );
    let extra = &db.query("SELECT extra_json FROM sessions", &[]).unwrap()[0][0];
    assert!(!extra.to_text().unwrap().contains("print(1+1)"));
    assert!(!extra.to_text().unwrap().contains("calculate"));
    db.query(
        "UPDATE messages SET content='edited via SQL' WHERE position=0",
        &[],
    )
    .unwrap();
    db.query(
        "UPDATE provider_requests SET output_tokens=99 WHERE request_id='r1'",
        &[],
    )
    .unwrap();
    let restored = ContentDatabase::memory(&db.export().unwrap()).unwrap();
    let loaded = read_json(&restored, "", "session_state.json");
    assert_eq!(loaded["messages"][0]["content"], "edited via SQL");
    assert_eq!(
        loaded["accounting"]["requests"][0]["usage"]["output_tokens"],
        99
    );
    assert!(
        restored
            .query("PRAGMA foreign_key_check", &[])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn replacement_rollback_and_cascades_preserve_session_integrity() {
    let db = ContentDatabase::memory(&[]).unwrap();
    let session = native_session();
    db.put(
        "",
        "session_state.json",
        &serde_json::to_vec(&session).unwrap(),
    )
    .unwrap();
    let mut invalid = session.clone();
    invalid["messages"][1]["tool_calls"][0]["function"]["name"] = json!(4);
    assert!(
        db.put(
            "",
            "session_state.json",
            &serde_json::to_vec(&invalid).unwrap()
        )
        .is_err()
    );
    assert_eq!(read_json(&db, "", "session_state.json"), session);
    db.put(
        "",
        "session_state.json",
        br#"{"messages":[],"blocks":[],"history":[]}"#,
    )
    .unwrap();
    for table in [
        "messages",
        "turns",
        "tool_calls",
        "message_parts",
        "provider_requests",
        "transcript_blocks",
        "input_history",
        "usage_totals",
    ] {
        assert_eq!(count(&db, table), 0, "{table}");
    }
    assert_eq!(count(&db, "sessions"), 1);
    db.remove_scope("").unwrap();
    assert_eq!(count(&db, "sessions"), 0);
}

#[test]
fn browser_relations_preserve_optional_shapes_and_binary_files() {
    let db = ContentDatabase::memory(&[]).unwrap();
    let state = json!({"id":"browser-id","name":null,"label":"test","updatedAt":"2026-10-23T00:00:00Z","system":"hello","python":true,"theme":"light","mode":"python","networkAccess":false,
 "connection":{"name":"local","provider":"openai","endpoint":"https://example.invalid/v1/chat/completions","model":"test","thinking":"enabled"},
 "messages":[{"role":"user","content":"hi"}],"inputs":["hi"],
 "blocks":[{"kind":"user","content":"hi","tool":null,"title":null,"usage":{"output_tokens":"12","total_tokens":"18446744073709551615","breakdown_complete":true},"estimated_cost":null}],
 "workspace":{"version":1,"directories":["empty"],"files":[{"path":"raw.bin","modifiedAt":123}]},
 "metrics":{"latestUsage":{"input":7,"output":12,"total":19,"cached":null,"cacheWrite":null,"reasoning":null},"context":{"tokens":7,"source":"server_usage"},"turn":{"requests":1,"responses":1,"sums":{"input":null,"output":12,"total":null,"cached":null,"cacheWrite":null,"reasoning":null},"reports":{"output":1}},"modelMs":1.25,"toolMs":0,"elapsedMs":2.5}});
    db.put(
        "browser/sessions",
        "browser-id",
        &serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    db.put("browser/files/browser-id", "raw.bin", &[0, 255, 1])
        .unwrap();
    assert_eq!(read_json(&db, "browser/sessions", "browser-id"), state);
    assert_eq!(
        db.query(
            "SELECT content,length(content) FROM files WHERE path='raw.bin'",
            &[]
        )
        .unwrap()[0][0]
            .to_blob(),
        Some(&[0, 255, 1][..])
    );
    assert_eq!(
        db.query("SELECT output_tokens FROM transcript_blocks", &[])
            .unwrap()[0][0]
            .as_int(),
        Some(12)
    );
    assert_eq!(
        db.query("SELECT network_access FROM sessions", &[])
            .unwrap()[0][0]
            .as_int(),
        Some(0)
    );
    assert!(
        db.query("PRAGMA foreign_key_check", &[])
            .unwrap()
            .is_empty()
    );
}

fn legacy_image(state: &[u8]) -> Vec<u8> {
    let db = ContentDatabase::memory(&[]).unwrap();
    db.connection.execute("PRAGMA foreign_keys=OFF").unwrap();
    for row in db
        .query("SELECT name FROM sqlite_schema WHERE type='view'", &[])
        .unwrap()
    {
        db.connection
            .execute(format!("DROP VIEW {}", row[0].to_text().unwrap()))
            .unwrap();
    }
    for row in db
        .query("SELECT name FROM sqlite_schema WHERE type='table'", &[])
        .unwrap()
    {
        db.connection
            .execute(format!("DROP TABLE {}", row[0].to_text().unwrap()))
            .unwrap();
    }
    db.connection.execute("CREATE TABLE content_records(scope TEXT NOT NULL,name TEXT NOT NULL,ordinal INTEGER NOT NULL,content BLOB NOT NULL,PRIMARY KEY(scope,name,ordinal)); PRAGMA user_version=1;").unwrap();
    db.query(
        "INSERT INTO content_records VALUES('','session_state.json',0,?)",
        &[Value::from_slice(state).unwrap()],
    )
    .unwrap();
    db.export().unwrap()
}
#[test]
fn v1_blob_database_migrates_and_invalid_source_is_not_silently_accepted() {
    let session = native_session();
    let bytes = legacy_image(&serde_json::to_vec(&session).unwrap());
    let migrated = ContentDatabase::memory(&bytes).unwrap();
    assert_eq!(read_json(&migrated, "", "session_state.json"), session);
    assert_eq!(count(&migrated, "messages"), 5);
    assert!(
        migrated
            .query(
                "SELECT name FROM sqlite_schema WHERE name='content_records'",
                &[]
            )
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        migrated.query("PRAGMA user_version", &[]).unwrap()[0][0].as_int(),
        Some(4)
    );
    assert!(ContentDatabase::memory(&legacy_image(b"not json")).is_err());
}
#[test]
fn unsigned_counters_remain_lossless_and_todos_are_relational() {
    let db = ContentDatabase::memory(&[]).unwrap();
    let state = json!({"accounting":{"requests":[{"usage":{"output_tokens":u64::MAX}}]}});
    db.put(
        "",
        "session_state.json",
        &serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    assert_eq!(read_json(&db, "", "session_state.json"), state);
    let todos = json!({"revision":u64::MAX,"todos":[{"id":"t1","content":"query data","status":"in_progress","priority":"high"}]});
    db.put("", "todos.json", &serde_json::to_vec(&todos).unwrap())
        .unwrap();
    assert_eq!(read_json(&db, "", "todos.json"), todos);
    assert_eq!(
        db.query("SELECT content FROM todos WHERE status='in_progress'", &[])
            .unwrap()[0][0]
            .to_text(),
        Some("query data")
    );
}

#[test]
fn provider_traces_are_normalized_and_v2_upgrade_preserves_conversations() {
    let db = ContentDatabase::memory(&[]).unwrap();
    let mut state = native_session();
    db.put(
        "",
        "session_state.json",
        &serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    // Construct an actual v2 image, with no trace table, from the shared schema.
    db.connection
        .execute("DROP TABLE provider_traces; PRAGMA user_version=2")
        .unwrap();
    let old = db.export().unwrap();
    let upgraded = ContentDatabase::memory(&old).unwrap();
    assert_eq!(read_json(&upgraded, "", "session_state.json"), state);
    assert_eq!(
        upgraded.query("PRAGMA user_version", &[]).unwrap()[0][0].as_int(),
        Some(4)
    );
    state["providerTrace"] = json!({"version":1,"omittedRequests":2,"requests":[{
        "id":"attempt-1","turnId":"turn-1","round":2,"attempt":1,"provider":"openai","model":"test",
        "startedAt":"2026-10-23T00:00:00Z","endedAt":null,"outcome":"error","httpStatus":200,"contentType":"text/event-stream",
        "requestBody":"{}","requestBytes":2,"requestOmittedBytes":0,"responseHead":"data: malformed","responseTail":"",
        "receivedBytes":15,"responseOmittedBytes":0,"responseComplete":false,"redacted":true,"error":"invalid arguments"}]});
    upgraded
        .put(
            "",
            "session_state.json",
            &serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
    assert_eq!(read_json(&upgraded, "", "session_state.json"), state);
    let rows = upgraded
        .query(
            "SELECT turn_id,http_status,response_head FROM provider_traces WHERE outcome='error'",
            &[],
        )
        .unwrap();
    assert_eq!(rows[0][0].to_text(), Some("turn-1"));
    assert_eq!(rows[0][1].as_int(), Some(200));
    assert_eq!(rows[0][2].to_text(), Some("data: malformed"));
    assert!(
        !upgraded
            .query("SELECT extra_json FROM sessions", &[])
            .unwrap()[0][0]
            .to_text()
            .unwrap()
            .contains("data: malformed")
    );
    upgraded
        .query("UPDATE provider_traces SET error='edited via SQL'", &[])
        .unwrap();
    assert_eq!(
        read_json(&upgraded, "", "session_state.json")["providerTrace"]["requests"][0]["error"],
        "edited via SQL"
    );
    upgraded.query("DELETE FROM records", &[]).unwrap();
    assert_eq!(count(&upgraded, "provider_traces"), 0);
}

#[test]
fn v3_adds_queryable_network_mode_and_preserves_session() {
    let db = ContentDatabase::memory(&[]).unwrap();
    // Recreate only the empty sessions table with its actual v3 columns.
    db.connection
        .execute("PRAGMA foreign_keys=OFF; DROP TABLE sessions")
        .unwrap();
    let schema = include_str!("schema.sql");
    let start = schema.find("CREATE TABLE sessions (").unwrap();
    let end = start + schema[start..].find(");").unwrap() + 2;
    db.connection
        .execute(schema[start..end].replace("    network_mode TEXT,\n", ""))
        .unwrap();
    db.connection
        .execute("PRAGMA user_version=3; PRAGMA foreign_keys=ON")
        .unwrap();
    let migrated = ContentDatabase::memory(&db.export().unwrap()).unwrap();
    let state = serde_json::json!({"id":"tor-session","name":null,"label":"Tor","updatedAt":"now","system":"retain","python":true,"theme":"dark","mode":"python","networkAccess":true,"networkMode":"tor","blocks":[],"messages":[],"inputs":[],"connection":null});
    migrated
        .put(
            "browser/sessions",
            "tor-session",
            &serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
    assert_eq!(
        read_json(&migrated, "browser/sessions", "tor-session"),
        state
    );
    assert_eq!(
        migrated
            .query("SELECT network_mode FROM sessions", &[])
            .unwrap()[0][0]
            .to_text(),
        Some("tor")
    );
}
