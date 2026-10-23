//! Lossless mapping at the JSON compatibility boundary. SQL columns are the
//! authority: only field presence/types and unmodeled extension fields are JSON.
use super::{ContentDatabase, message};
use serde_json::{Map, Value as Json};
use turso_core::Value;

pub(super) fn text(value: &str) -> Value {
    Value::build_text(value.to_owned())
}
pub(super) fn integer(value: usize) -> Result<Value, String> {
    i64::try_from(value).map(Value::from_i64).map_err(message)
}
pub(super) fn take_path(value: &mut Json, path: &str) -> Option<Json> {
    if let Some((head, tail)) = path.split_once('.') {
        take_path(value.get_mut(head)?, tail)
    } else {
        value.as_object_mut()?.remove(path)
    }
}
fn set_path(value: &mut Json, path: &str, field: Json) -> Result<(), String> {
    if let Some((head, tail)) = path.split_once('.') {
        let child = value
            .as_object_mut()
            .ok_or("invalid object shape")?
            .entry(head)
            .or_insert_with(|| Json::Object(Map::new()));
        set_path(child, tail, field)
    } else {
        value
            .as_object_mut()
            .ok_or("invalid object shape")?
            .insert(path.to_owned(), field);
        Ok(())
    }
}
fn descriptor(field: &str) -> (&str, &str, &str) {
    let (column, rest) = field.split_once('=').expect("static column descriptor");
    let (path, kind) = rest.rsplit_once(':').expect("static field descriptor");
    (column, path, kind)
}
fn sql_value(value: &Json, kind: &str) -> Result<Value, String> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    match kind {
        "s" => value
            .as_str()
            .map(text)
            .ok_or_else(|| "expected text field".to_owned()),
        "b" => value
            .as_bool()
            .map(|v| Value::from_i64(i64::from(v)))
            .ok_or_else(|| "expected boolean field".to_owned()),
        "j" => Ok(text(&value.to_string())),
        "n" => {
            if let Some(n) = value.as_i64() {
                Ok(Value::from_i64(n))
            }
            // Preserve u64 values beyond SQLite's signed integer range exactly.
            // BLOB affinity prevents INTEGER affinity rounding them to floats.
            else if value.as_u64().is_some() {
                Value::from_slice(value.to_string().as_bytes()).map_err(message)
            } else if let Some(n) = value.as_f64() {
                Ok(Value::from_f64(n))
            }
            // WFE contracts encode counters as decimal strings.
            else if let Some(s) = value.as_str() {
                // Canonical WFE integer strings stay numeric in SQL. Other
                // representations must not be rounded/coerced by affinity.
                if let Ok(n) = s.parse::<i64>()
                    && n.to_string() == s
                {
                    Ok(Value::from_i64(n))
                } else {
                    Value::from_slice(s.as_bytes()).map_err(message)
                }
            } else {
                Err("expected numeric field".to_owned())
            }
        }
        _ => Err("invalid field descriptor".to_owned()),
    }
}
fn json_value(value: &Value, kind: &str, original: &str) -> Result<Json, String> {
    if matches!(value, Value::Null) {
        return Ok(Json::Null);
    }
    let scalar = match value {
        Value::Numeric(_) => value
            .as_int()
            .map_or_else(|| value.as_float().to_string(), |n| n.to_string()),
        _ => value
            .to_text()
            .map(str::to_owned)
            .or_else(|| {
                value
                    .to_blob()
                    .and_then(|v| String::from_utf8(v.to_vec()).ok())
            })
            .ok_or("invalid SQL field value")?,
    };
    match kind {
        "s" => Ok(Json::String(scalar)),
        "b" => match scalar.as_str() {
            "0" => Ok(Json::Bool(false)),
            "1" => Ok(Json::Bool(true)),
            _ => Err("invalid stored boolean".to_owned()),
        },
        "n" if original == "string" => Ok(Json::String(scalar)),
        "n" | "j" => serde_json::from_str(&scalar).map_err(message),
        _ => Err("invalid stored field type".to_owned()),
    }
}

pub(super) fn insert(
    db: &ContentDatabase,
    table: &str,
    base: &[(&str, Value)],
    fields: &[&str],
    value: &Json,
) -> Result<i64, String> {
    if !value.is_object() {
        return Err(format!("{table} requires an object"));
    }
    let mut extra = value.clone();
    let mut shape = Map::new();
    let mut columns: Vec<&str> = base.iter().map(|(key, _)| *key).collect();
    let mut values: Vec<Value> = base.iter().map(|(_, value)| value.clone()).collect();
    for field in fields {
        let (column, path, kind) = descriptor(field);
        columns.push(column);
        let value = take_path(&mut extra, path);
        if let Some(value) = &value {
            shape.insert(
                path.to_owned(),
                Json::String(if value.is_string() { "string" } else { "value" }.to_owned()),
            );
        }
        values.push(
            sql_value(value.as_ref().unwrap_or(&Json::Null), kind)
                .map_err(|error| format!("{table}.{column}: {error}"))?,
        );
    }
    columns.extend(["fields_json", "extra_json"]);
    values.extend([
        text(&Json::Object(shape).to_string()),
        text(&extra.to_string()),
    ]);
    let placeholders = vec!["?"; values.len()].join(",");
    let returning = if base
        .iter()
        .any(|(k, _)| *k == "position" || *k == "category")
    {
        "id"
    } else {
        "record_id"
    };
    let sql = format!(
        "INSERT INTO {table} ({}) VALUES ({placeholders}) RETURNING {returning}",
        columns.join(",")
    );
    db.query(&sql, &values)?
        .first()
        .and_then(|r| r.first())
        .and_then(Value::as_int)
        .ok_or_else(|| format!("{table}: insert returned no identity"))
}

pub(super) fn select(
    db: &ContentDatabase,
    table: &str,
    fields: &[&str],
    filter: &str,
    args: &[Value],
) -> Result<Vec<(i64, Json)>, String> {
    let identity = if matches!(table, "sessions" | "connections" | "todo_lists") {
        "record_id"
    } else {
        "id"
    };
    let columns = fields
        .iter()
        .map(|field| descriptor(field).0)
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT {identity},fields_json,extra_json,{columns} FROM {table} {filter}");
    db.query(&sql, args)?
        .into_iter()
        .map(|row| {
            let id = row[0].as_int().ok_or("invalid row identity")?;
            let shape: Map<String, Json> =
                serde_json::from_str(row[1].to_text().ok_or("invalid field shape")?)
                    .map_err(message)?;
            let mut value: Json =
                serde_json::from_str(row[2].to_text().ok_or("invalid extension fields")?)
                    .map_err(message)?;
            for (field, data) in fields.iter().zip(&row[3..]) {
                let (_, path, kind) = descriptor(field);
                if let Some(original) = shape.get(path) {
                    set_path(
                        &mut value,
                        path,
                        json_value(data, kind, original.as_str().ok_or("invalid field type")?)?,
                    )?;
                }
            }
            Ok((id, value))
        })
        .collect()
}

pub(super) fn detach_array(value: &mut Json, path: &str) -> Result<Option<Vec<Json>>, String> {
    let Some(array) = take_path(value, path) else {
        return Ok(None);
    };
    if array.is_null() {
        set_path(value, path, Json::Null)?;
        return Ok(None);
    }
    let Json::Array(array) = array else {
        return Err(format!("{path} must be an array"));
    };
    set_path(value, path, Json::Array(Vec::new()))?;
    Ok(Some(array))
}
pub(super) fn restore_array(value: &mut Json, path: &str, array: Vec<Json>) -> Result<(), String> {
    // Missing and null collections must stay distinguishable from empty arrays.
    let pointer = format!("/{}", path.replace('.', "/"));
    if value.pointer(&pointer).is_some_and(Json::is_array) {
        set_path(value, path, Json::Array(array))?;
    }
    Ok(())
}
