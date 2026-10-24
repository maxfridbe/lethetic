//! Domain persistence. JSON is reconstructed only at existing application API
//! boundaries; messages, calls, blocks, accounting and files live in SQL rows.
use super::{
    ContentDatabase, fields as f, message,
    rows::{self, integer, text},
};
use serde_json::Value as Json;
use turso_core::Value;

fn base(id: i64, position: usize) -> Result<Vec<(&'static str, Value)>, String> {
    Ok(vec![
        ("record_id", Value::from_i64(id)),
        ("position", integer(position)?),
    ])
}
fn items(db: &ContentDatabase, table: &str, fields: &[&str], id: i64) -> Result<Vec<Json>, String> {
    Ok(rows::select(
        db,
        table,
        fields,
        "WHERE record_id=? ORDER BY position",
        &[Value::from_i64(id)],
    )?
    .into_iter()
    .map(|(_, v)| v)
    .collect())
}
fn save_items(
    db: &ContentDatabase,
    table: &str,
    fields: &[&str],
    id: i64,
    values: Option<Vec<Json>>,
) -> Result<(), String> {
    for (position, value) in values.unwrap_or_default().iter().enumerate() {
        rows::insert(db, table, &base(id, position)?, fields, value)?;
    }
    Ok(())
}
fn save_strings(
    db: &ContentDatabase,
    table: &str,
    column: &str,
    id: i64,
    values: Option<Vec<Json>>,
) -> Result<(), String> {
    for (position, value) in values.unwrap_or_default().iter().enumerate() {
        let value = value.as_str().ok_or("expected string array")?;
        db.query(
            &format!("INSERT INTO {table}(record_id,position,{column}) VALUES(?,?,?)"),
            &[Value::from_i64(id), integer(position)?, text(value)],
        )?;
    }
    Ok(())
}
fn strings(db: &ContentDatabase, table: &str, column: &str, id: i64) -> Result<Vec<Json>, String> {
    db.query(
        &format!("SELECT {column} FROM {table} WHERE record_id=? ORDER BY position"),
        &[Value::from_i64(id)],
    )?
    .into_iter()
    .map(|r| {
        r[0].to_text()
            .map(|v| Json::String(v.to_owned()))
            .ok_or_else(|| "invalid string row".to_owned())
    })
    .collect()
}

fn save_messages(db: &ContentDatabase, id: i64, messages: Option<Vec<Json>>) -> Result<(), String> {
    let mut turn = None;
    for (position, mut value) in messages.unwrap_or_default().into_iter().enumerate() {
        if value.get("role").and_then(Json::as_str) == Some("user") {
            let next = turn.map_or(0_i64, |v| v + 1);
            db.query(
                "INSERT INTO turns(record_id,turn_index,first_message_position) VALUES(?,?,?)",
                &[
                    Value::from_i64(id),
                    Value::from_i64(next),
                    integer(position)?,
                ],
            )?;
            turn = Some(next);
        }
        let calls = rows::detach_array(&mut value, "tool_calls")?;
        let parts = rows::detach_array(&mut value, "provider_content")?;
        let mut fields = base(id, position)?;
        fields.push(("turn_index", turn.map_or(Value::Null, Value::from_i64)));
        let result_call_id = value
            .get("tool_call_id")
            .and_then(Json::as_str)
            .map(str::to_owned)
            .or_else(|| {
                if value.get("role").and_then(Json::as_str) == Some("tool") {
                    value
                        .get("content")
                        .and_then(Json::as_str)
                        .and_then(crate::context::ContextManager::extract_tool_result_fields)
                        .map(|(id, _)| id)
                } else {
                    None
                }
            });
        fields.push((
            "result_call_id",
            result_call_id.as_deref().map_or(Value::Null, text),
        ));
        let message_id = rows::insert(db, "messages", &fields, f::MESSAGE, &value)?;
        for (table, fields, values) in [
            ("tool_calls", f::CALL, calls),
            ("message_parts", f::PART, parts),
        ] {
            for (index, value) in values.unwrap_or_default().iter().enumerate() {
                rows::insert(
                    db,
                    table,
                    &[
                        ("message_id", Value::from_i64(message_id)),
                        ("position", integer(index)?),
                    ],
                    fields,
                    value,
                )?;
            }
        }
    }
    Ok(())
}
fn messages(db: &ContentDatabase, id: i64) -> Result<Vec<Json>, String> {
    rows::select(
        db,
        "messages",
        f::MESSAGE,
        "WHERE record_id=? ORDER BY position",
        &[Value::from_i64(id)],
    )?
    .into_iter()
    .map(|(message_id, mut value)| {
        for (table, fields, path) in [
            ("tool_calls", f::CALL, "tool_calls"),
            ("message_parts", f::PART, "provider_content"),
        ] {
            let array = rows::select(
                db,
                table,
                fields,
                "WHERE message_id=? ORDER BY position",
                &[Value::from_i64(message_id)],
            )?
            .into_iter()
            .map(|(_, v)| v)
            .collect();
            rows::restore_array(&mut value, path, array)?;
        }
        Ok(value)
    })
    .collect()
}

fn save_totals(
    db: &ContentDatabase,
    id: i64,
    state: &mut Json,
    browser: bool,
) -> Result<(), String> {
    let categories: &[(&str, &str)] = if browser {
        &[
            ("turn", "turn"),
            ("session", "session"),
            ("latest_response", "latestUsage"),
        ]
    } else {
        &[
            ("latest_logical_turn", "latest_logical_turn"),
            ("session", "session"),
        ]
    };
    for &(category, field) in categories {
        let fields = if category == "latest_response" {
            f::BROWSER_LATEST.to_vec()
        } else {
            f::total(browser)
        };
        let parent = if browser { "metrics" } else { "accounting" };
        if let Some(total) = state.get_mut(parent).and_then(|v| v.get_mut(field)) {
            if total.is_null() {
                continue;
            }
            rows::insert(
                db,
                "usage_totals",
                &[
                    ("record_id", Value::from_i64(id)),
                    ("category", text(category)),
                ],
                &fields,
                total,
            )?;
            *total = serde_json::json!({});
        }
    }
    Ok(())
}
fn restore_totals(
    db: &ContentDatabase,
    id: i64,
    state: &mut Json,
    browser: bool,
) -> Result<(), String> {
    let categories: &[(&str, &str)] = if browser {
        &[
            ("turn", "turn"),
            ("session", "session"),
            ("latest_response", "latestUsage"),
        ]
    } else {
        &[
            ("latest_logical_turn", "latest_logical_turn"),
            ("session", "session"),
        ]
    };
    for &(category, field) in categories {
        let fields = if category == "latest_response" {
            f::BROWSER_LATEST.to_vec()
        } else {
            f::total(browser)
        };
        if let Some((_, value)) = rows::select(
            db,
            "usage_totals",
            &fields,
            "WHERE record_id=? AND category=?",
            &[Value::from_i64(id), text(category)],
        )?
        .into_iter()
        .next()
        {
            state[if browser { "metrics" } else { "accounting" }][field] = value;
        }
    }
    Ok(())
}

pub(super) fn save_json(
    db: &ContentDatabase,
    id: i64,
    kind: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let mut state: Json =
        serde_json::from_slice(bytes).map_err(|error| format!("invalid {kind}: {error}"))?;
    if kind == "todos" && state.is_array() {
        state = serde_json::json!({"revision": 0, "todos": state});
    }
    match kind {
        "native_session" | "browser_session" => {
            let browser = kind == "browser_session";
            save_messages(db, id, rows::detach_array(&mut state, "messages")?)?;
            save_items(
                db,
                "provider_traces",
                f::PROVIDER_TRACE,
                id,
                rows::detach_array(&mut state, "providerTrace.requests")?,
            )?;
            save_items(
                db,
                "transcript_blocks",
                &f::block(browser),
                id,
                rows::detach_array(&mut state, "blocks")?,
            )?;
            save_strings(
                db,
                "input_history",
                "content",
                id,
                rows::detach_array(&mut state, if browser { "inputs" } else { "history" })?,
            )?;
            save_items(
                db,
                "provider_requests",
                &f::request(),
                id,
                rows::detach_array(&mut state, "accounting.requests")?,
            )?;
            save_items(
                db,
                "file_entries",
                f::FILE,
                id,
                rows::detach_array(&mut state, "workspace.files")?,
            )?;
            save_strings(
                db,
                "directories",
                "path",
                id,
                rows::detach_array(&mut state, "workspace.directories")?,
            )?;
            save_totals(db, id, &mut state, browser)?;
            if let Some(counts) = state
                .get_mut("tool_use_counts")
                .and_then(Json::as_object_mut)
            {
                for (name, count) in std::mem::take(counts) {
                    let count = count.as_u64().ok_or("invalid tool count")?;
                    db.query(
                        "INSERT INTO tool_statistics(record_id,tool_name,call_count) VALUES(?,?,?)",
                        &[Value::from_i64(id), text(&name), text(&count.to_string())],
                    )?;
                }
            }
            if browser
                && let Some(connection) = state.get_mut("connection")
                && !connection.is_null()
            {
                // Connection credentials never belong in the session database.
                if let Some(object) = connection.as_object_mut() {
                    object.remove("apiKey");
                }
                rows::insert(
                    db,
                    "connections",
                    &[("record_id", Value::from_i64(id))],
                    f::CONNECTION,
                    connection,
                )?;
                *connection = serde_json::json!({});
            }
            rows::insert(
                db,
                "sessions",
                &[("record_id", Value::from_i64(id))],
                if browser {
                    f::BROWSER_SESSION
                } else {
                    f::NATIVE_SESSION
                },
                &state,
            )?;
        }
        "messages" => save_messages(
            db,
            id,
            Some(state.as_array().ok_or("context must be an array")?.clone()),
        )?,
        "blocks" => save_items(
            db,
            "transcript_blocks",
            &f::block(false),
            id,
            Some(
                state
                    .as_array()
                    .ok_or("transcript must be an array")?
                    .clone(),
            ),
        )?,
        "history" => save_strings(
            db,
            "input_history",
            "content",
            id,
            Some(state.as_array().ok_or("history must be an array")?.clone()),
        )?,
        "todos" => {
            save_items(
                db,
                "todos",
                f::TODO,
                id,
                rows::detach_array(&mut state, "todos")?,
            )?;
            rows::insert(
                db,
                "todo_lists",
                &[("record_id", Value::from_i64(id))],
                &["revision=revision:n"],
                &state,
            )?;
        }
        _ => return Err(format!("unsupported structured record {kind}")),
    }
    Ok(())
}
pub(super) fn read_json(db: &ContentDatabase, id: i64, kind: &str) -> Result<Vec<u8>, String> {
    let result = match kind {
        "native_session" | "browser_session" => {
            let browser = kind == "browser_session";
            let mut state = rows::select(
                db,
                "sessions",
                if browser {
                    f::BROWSER_SESSION
                } else {
                    f::NATIVE_SESSION
                },
                "WHERE record_id=?",
                &[Value::from_i64(id)],
            )?
            .into_iter()
            .next()
            .ok_or("missing session row")?
            .1;
            rows::restore_array(&mut state, "messages", messages(db, id)?)?;
            rows::restore_array(
                &mut state,
                "providerTrace.requests",
                items(db, "provider_traces", f::PROVIDER_TRACE, id)?,
            )?;
            rows::restore_array(
                &mut state,
                "blocks",
                items(db, "transcript_blocks", &f::block(browser), id)?,
            )?;
            rows::restore_array(
                &mut state,
                if browser { "inputs" } else { "history" },
                strings(db, "input_history", "content", id)?,
            )?;
            rows::restore_array(
                &mut state,
                "accounting.requests",
                items(db, "provider_requests", &f::request(), id)?,
            )?;
            rows::restore_array(
                &mut state,
                "workspace.files",
                items(db, "file_entries", f::FILE, id)?,
            )?;
            rows::restore_array(
                &mut state,
                "workspace.directories",
                strings(db, "directories", "path", id)?,
            )?;
            restore_totals(db, id, &mut state, browser)?;
            if let Some(counts) = state
                .get_mut("tool_use_counts")
                .and_then(Json::as_object_mut)
            {
                for row in db.query(
                    "SELECT tool_name,call_count FROM tool_statistics WHERE record_id=?",
                    &[Value::from_i64(id)],
                )? {
                    counts.insert(
                        row[0].to_text().ok_or("invalid tool name")?.to_owned(),
                        serde_json::from_str(row[1].to_text().ok_or("invalid tool count")?)
                            .map_err(message)?,
                    );
                }
            }
            if browser
                && let Some((_, connection)) = rows::select(
                    db,
                    "connections",
                    f::CONNECTION,
                    "WHERE record_id=?",
                    &[Value::from_i64(id)],
                )?
                .into_iter()
                .next()
            {
                state["connection"] = connection;
            }
            state
        }
        "messages" => Json::Array(messages(db, id)?),
        "blocks" => Json::Array(items(db, "transcript_blocks", &f::block(false), id)?),
        "history" => Json::Array(strings(db, "input_history", "content", id)?),
        "todos" => {
            let mut state = rows::select(
                db,
                "todo_lists",
                &["revision=revision:n"],
                "WHERE record_id=?",
                &[Value::from_i64(id)],
            )?
            .into_iter()
            .next()
            .ok_or("missing todo revision")?
            .1;
            rows::restore_array(&mut state, "todos", items(db, "todos", f::TODO, id)?)?;
            state
        }
        _ => return Err("unsupported structured record".to_owned()),
    };
    serde_json::to_vec(&result).map_err(message)
}
