//! Identity catalog and legacy API adaptation. Structured records are routed to
//! relational tables; only opaque artifacts use binary chunks.
use super::{ContentDatabase, keys, message, relational, rows::text};
use turso_core::Value;

impl ContentDatabase {
    pub(super) fn record(&self, scope: &str, name: &str) -> Result<Option<(i64, String)>, String> {
        self.query(
            "SELECT id,kind FROM records WHERE scope=? AND name=?",
            &keys(scope, name),
        )?
        .into_iter()
        .next()
        .map(|row| {
            Ok((
                row[0].as_int().ok_or("invalid record id")?,
                row[1].to_text().ok_or("invalid record kind")?.to_owned(),
            ))
        })
        .transpose()
    }
    pub fn contains(&self, scope: &str, name: &str) -> Result<bool, String> {
        Ok(self.record(scope, name)?.is_some())
    }
    pub fn read(&self, scope: &str, name: &str) -> Result<Option<Vec<u8>>, String> {
        let Some((id, kind)) = self.record(scope, name)? else {
            return Ok(None);
        };
        if kind == "file" {
            let rows = self.query(
                "SELECT content FROM files WHERE record_id=?",
                &[Value::from_i64(id)],
            )?;
            return rows
                .first()
                .and_then(|r| r[0].to_blob())
                .map(|b| Some(b.to_vec()))
                .ok_or_else(|| "missing file contents".to_owned());
        }
        if kind != "text" && kind != "binary" {
            return relational::read_json(self, id, &kind).map(Some);
        }
        let table = if kind == "text" {
            "text_chunks"
        } else {
            "binary_chunks"
        };
        let rows = self.query(
            &format!("SELECT content FROM {table} WHERE record_id=? ORDER BY ordinal"),
            &[Value::from_i64(id)],
        )?;
        let mut output = Vec::new();
        for row in rows {
            let data = if kind == "text" {
                row[0].to_text().map(str::as_bytes)
            } else {
                row[0].to_blob()
            }
            .ok_or("invalid artifact chunk")?;
            output.try_reserve(data.len()).map_err(message)?;
            output.extend_from_slice(data);
        }
        Ok(Some(output))
    }
    /// Replaces the domain rows atomically. Readers reconstruct the compatibility
    /// transport from these rows, not from a duplicate serialized session.
    pub fn put(&self, scope: &str, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.transaction(|| {
            self.remove(scope, name)?;
            self.append_in_transaction(scope, name, bytes)
        })
    }
    pub fn append(&self, scope: &str, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.transaction(|| self.append_in_transaction(scope, name, bytes))
    }
    /// May be called inside a host-owned transaction during legacy migration.
    pub(super) fn append_in_transaction(
        &self,
        scope: &str,
        name: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let existing = self.record(scope, name)?;
        let (id, kind) = if let Some((id, kind)) = existing {
            (id, kind)
        } else {
            let kind = record_kind(scope, name, bytes);
            let row = self.query(
                "INSERT INTO records(scope,name,kind) VALUES(?,?,?) RETURNING id",
                &[text(scope), text(name), text(kind)],
            )?;
            (
                row[0][0].as_int().ok_or("missing record identity")?,
                kind.to_owned(),
            )
        };
        if kind == "file" {
            self.query(
                "INSERT INTO files(record_id,path,content) VALUES(?,?,?)",
                &[
                    Value::from_i64(id),
                    text(name),
                    Value::from_slice(bytes).map_err(message)?,
                ],
            )?;
        } else if kind == "text" || kind == "binary" {
            let (table, data) = if kind == "text" {
                (
                    "text_chunks",
                    text(std::str::from_utf8(bytes).map_err(message)?),
                )
            } else {
                ("binary_chunks", Value::from_slice(bytes).map_err(message)?)
            };
            self.query(&format!("INSERT INTO {table}(record_id,ordinal,content) SELECT ?1,COALESCE(MAX(ordinal)+1,0),?2 FROM {table} WHERE record_id=?1"), &[Value::from_i64(id),data])?;
        } else {
            relational::save_json(self, id, &kind, bytes)?;
        }
        Ok(())
    }
    pub fn remove(&self, scope: &str, name: &str) -> Result<(), String> {
        self.query(
            "DELETE FROM records WHERE scope=? AND name=?",
            &keys(scope, name),
        )?;
        Ok(())
    }
    pub fn remove_scope(&self, scope: &str) -> Result<(), String> {
        self.query("DELETE FROM records WHERE scope=?", &[text(scope)])?;
        Ok(())
    }
    pub fn names(&self, scope: &str) -> Result<Vec<String>, String> {
        self.query(
            "SELECT name FROM records WHERE scope=? ORDER BY name",
            &[text(scope)],
        )?
        .into_iter()
        .map(|row| {
            row[0]
                .to_text()
                .map(str::to_owned)
                .ok_or_else(|| "invalid record name".to_owned())
        })
        .collect()
    }
    pub(super) fn migrate_v1(&self) -> Result<(), String> {
        // Keep the source table until every record has been parsed and inserted.
        // Any malformed record rolls the entire schema migration back.
        let mut documents = std::collections::BTreeMap::<(String, String), Vec<u8>>::new();
        for row in self.query(
            "SELECT scope,name,content FROM content_records ORDER BY scope,name,ordinal",
            &[],
        )? {
            let key = (
                row[0].to_text().ok_or("invalid legacy scope")?.to_owned(),
                row[1].to_text().ok_or("invalid legacy name")?.to_owned(),
            );
            documents
                .entry(key)
                .or_default()
                .extend_from_slice(row[2].to_blob().ok_or("invalid legacy bytes")?);
        }
        for ((scope, name), bytes) in documents {
            self.append_in_transaction(&scope, &name, &bytes)?;
        }
        self.connection
            .execute("DROP TABLE content_records")
            .map_err(message)
    }
}
fn record_kind(scope: &str, name: &str, bytes: &[u8]) -> &'static str {
    if scope == "browser/sessions" {
        return "browser_session";
    }
    if scope.starts_with("browser/files/") {
        return "file";
    }
    // Named structured records are reserved only in session/project namespaces;
    // arbitrary tool artifacts with the same filename remain ordinary artifacts.
    if scope.is_empty() || scope.starts_with("sessions/") || scope.starts_with("python-sessions/") {
        match name {
            "session_state.json" => return "native_session",
            "context.json" => return "messages",
            "ui_state.json" => return "blocks",
            "history.json" => return "history",
            "todos.json" => return "todos",
            _ => {}
        }
    }
    if std::str::from_utf8(bytes).is_ok() {
        "text"
    } else {
        "binary"
    }
}
