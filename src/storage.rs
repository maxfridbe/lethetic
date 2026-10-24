//! Shared embedded Turso content store for native and browser hosts.
//! Host adapters own durability; SQL and the on-disk schema are identical.
use std::{num::NonZeroUsize, sync::Arc};
use turso_core::{
    Buffer, Completion, Connection, Database, IO, OpenFlags, OpenOptions, SqliteDialect, Value,
};

mod fields;
mod memory;
mod records;
mod relational;
mod rows;
pub use turso_core::{Clock as DatabaseClock, MonotonicInstant, WallClockInstant};

#[cfg(feature = "native")]
pub mod native;

pub const DATABASE_NAME: &str = "lethetic.db";
const SCHEMA_VERSION: i64 = 3;
const MAX_DATABASE_BYTES: usize = 512 * 1024 * 1024;

pub struct ContentDatabase {
    connection: Arc<Connection>,
    io: Arc<dyn IO>,
    path: String,
}

impl ContentDatabase {
    pub fn open(io: Arc<dyn IO>, path: &str) -> Result<Self, String> {
        // Explicit storage avoids Turso's process-global path registry: browser
        // instances have independent MemoryIO files with the same logical name.
        let file = io
            .open_file(path, OpenFlags::Create, false)
            .map_err(message)?;
        let options = OpenOptions::new(Arc::new(SqliteDialect)).storage(Arc::new(
            turso_core::storage::database::DatabaseFile::new(file),
        ));
        let mut state = turso_core::OpenDbAsyncState::new();
        let database = loop {
            match Database::do_open_async(&mut state, io.clone(), path, &options)
                .map_err(message)?
            {
                turso_core::IOResult::Done(database) => break database,
                turso_core::IOResult::IO(completion) => {
                    completion.wait(io.as_ref()).map_err(message)?
                }
            }
        };
        let connection = database.connect().map_err(message)?;
        connection.set_temp_store(turso_core::TempStore::Memory);
        let store = Self {
            connection,
            io,
            path: path.to_owned(),
        };
        let rows = store.query("PRAGMA user_version", &[])?;
        let version = rows
            .first()
            .and_then(|row| row.first())
            .and_then(Value::as_int)
            .unwrap_or(-1);
        store
            .connection
            .execute("PRAGMA foreign_keys=ON")
            .map_err(message)?;
        if version == 0
            && !store
                .query("SELECT name FROM sqlite_schema WHERE type='table'", &[])?
                .is_empty()
        {
            return Err("unversioned database is not a Lethetic session".to_owned());
        }
        if (0..SCHEMA_VERSION).contains(&version) {
            store.transaction(|| {
                store
                    .connection
                    .execute(if version == 2 {
                        include_str!("storage/provider-traces.sql")
                    } else {
                        include_str!("storage/schema.sql")
                    })
                    .map_err(message)?;
                if version == 1 {
                    store.migrate_v1()?;
                }
                store
                    .connection
                    .execute("PRAGMA user_version=3")
                    .map_err(message)
            })?;
        } else if version != SCHEMA_VERSION {
            return Err(format!("unsupported content database schema {version}"));
        }
        Ok(store)
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn memory(bytes: &[u8]) -> Result<Self, String> {
        Self::memory_with_clock(bytes, Arc::new(turso_core::io::clock::DefaultClock))
    }

    pub fn memory_with_clock(
        bytes: &[u8],
        clock: Arc<dyn DatabaseClock + Send + Sync>,
    ) -> Result<Self, String> {
        if !bytes.is_empty()
            && (bytes.len() < 100
                || bytes.len() > MAX_DATABASE_BYTES
                || !bytes.starts_with(b"SQLite format 3\0"))
        {
            return Err("invalid or oversized content database".to_owned());
        }
        // Imported images must already carry our schema version. Only an empty
        // byte slice requests a new store; foreign/corrupt images never hydrate
        // as a blank replacement session database.
        if !bytes.is_empty()
            && ![
                1_u32.to_be_bytes(),
                2_u32.to_be_bytes(),
                (SCHEMA_VERSION as u32).to_be_bytes(),
            ]
            .contains(&bytes[60..64].try_into().map_err(message)?)
        {
            return Err("unsupported content database image schema".to_owned());
        }
        let io = Arc::new(memory::MemoryHost::new(clock));
        if !bytes.is_empty() {
            let file = io
                .open_file(DATABASE_NAME, OpenFlags::Create, false)
                .map_err(message)?;
            let completion = file
                .pwrite(
                    0,
                    Arc::new(Buffer::new(bytes.to_vec())),
                    Completion::new_write(|_| {}),
                )
                .map_err(message)?;
            io.wait_for_completion(completion).map_err(message)?;
        }
        Self::open(io, DATABASE_NAME)
    }

    pub fn query(&self, sql: &str, parameters: &[Value]) -> Result<Vec<Vec<Value>>, String> {
        let mut statement = self.connection.prepare(sql).map_err(message)?;
        for (index, value) in parameters.iter().enumerate() {
            statement
                .bind_at(
                    NonZeroUsize::new(index + 1).expect("one-based parameter index"),
                    value.clone(),
                )
                .map_err(message)?;
        }
        statement.run_collect_rows().map_err(message)
    }

    /// SQL access for embedding hosts. BLOB cells are returned as byte arrays.
    /// Mutations affect this database instance; hosts still own its durability.
    pub fn query_json(&self, sql: &str, parameters: &str) -> Result<String, String> {
        let parameters: Vec<serde_json::Value> =
            serde_json::from_str(parameters).map_err(message)?;
        let mut statement = self.connection.prepare(sql).map_err(message)?;
        for (index, value) in parameters.into_iter().enumerate() {
            let value = match value {
                serde_json::Value::Null => Value::Null,
                serde_json::Value::Bool(value) => Value::from_i64(i64::from(value)),
                serde_json::Value::String(value) => Value::build_text(value),
                serde_json::Value::Number(value) => {
                    if let Some(value) = value.as_i64() {
                        Value::from_i64(value)
                    } else if value.as_u64().is_some() {
                        return Err("SQL integer parameter exceeds signed 64-bit range".to_owned());
                    } else {
                        Value::from_f64(value.as_f64().ok_or("invalid SQL number")?)
                    }
                }
                _ => return Err("SQL parameters must be scalar values".to_owned()),
            };
            statement
                .bind_at(
                    NonZeroUsize::new(index + 1).expect("one-based parameter index"),
                    value,
                )
                .map_err(message)?;
        }
        let columns: Vec<String> = (0..statement.num_columns())
            .map(|index| statement.get_column_name(index).into_owned())
            .collect();
        let rows: Vec<Vec<serde_json::Value>> = statement
            .run_collect_rows()
            .map_err(message)?
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|value| match value {
                        Value::Null => serde_json::Value::Null,
                        Value::Numeric(_) => value.as_int().map_or_else(
                            || serde_json::json!(value.as_float()),
                            |value| serde_json::json!(value),
                        ),
                        Value::Text(_) => serde_json::json!(value.to_text()),
                        Value::Blob(_) => serde_json::json!(value.to_blob()),
                    })
                    .collect()
            })
            .collect();
        serde_json::to_string(&serde_json::json!({"columns":columns,"rows":rows})).map_err(message)
    }

    pub fn transaction<T>(
        &self,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.connection.execute("BEGIN").map_err(message)?;
        let result = operation().and_then(|result| {
            self.connection.execute("COMMIT").map_err(message)?;
            Ok(result)
        });
        if let Err(error) = &result
            && let Err(rollback) = self.connection.execute("ROLLBACK")
        {
            return Err(format!("{error}; rollback failed: {rollback}"));
        }
        result
    }

    /// A self-contained SQLite-compatible image, including committed WAL pages.
    pub fn export(&self) -> Result<Vec<u8>, String> {
        self.connection
            .checkpoint(turso_core::CheckpointMode::Truncate {
                upper_bound_inclusive: None,
            })
            .map_err(message)?;
        let file = self
            .io
            .open_file(&self.path, OpenFlags::default(), false)
            .map_err(message)?;
        let size = usize::try_from(file.size().map_err(message)?).map_err(message)?;
        if size > MAX_DATABASE_BYTES {
            return Err("content database exceeds 512 MiB".to_owned());
        }
        let buffer = Arc::new(Buffer::new_temporary(size));
        let completion = file
            .pread(0, Completion::new_read(buffer.clone(), |_| None))
            .map_err(message)?;
        self.io.wait_for_completion(completion).map_err(message)?;
        Ok(buffer.as_slice().to_vec())
    }

    pub fn close(&self) -> Result<(), String> {
        self.connection.close().map_err(message)
    }
}

fn keys(scope: &str, name: &str) -> [Value; 2] {
    [
        Value::build_text(scope.to_owned()),
        Value::build_text(name.to_owned()),
    ]
}
fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_image_preserves_chunks_binary_data_and_scope_isolation() {
        let db = ContentDatabase::memory(&[]).unwrap();
        db.put("one", "transcript", b"hello").unwrap();
        db.append("one", "transcript", b" world").unwrap();
        db.put("two", "transcript", &[0, 255, 1]).unwrap();
        let image = db.export().unwrap();
        assert!(image.starts_with(b"SQLite format 3\0"));
        let restored = ContentDatabase::memory(&image).unwrap();
        assert_eq!(
            restored.read("one", "transcript").unwrap().unwrap(),
            b"hello world"
        );
        assert_eq!(
            restored.read("two", "transcript").unwrap().unwrap(),
            [0, 255, 1]
        );
        restored.put("one", "transcript", b"replacement").unwrap();
        restored.remove_scope("two").unwrap();
        assert_eq!(
            restored.read("one", "transcript").unwrap().unwrap(),
            b"replacement"
        );
        assert!(restored.read("two", "transcript").unwrap().is_none());
    }

    #[test]
    fn failed_transaction_preserves_previous_data() {
        let db = ContentDatabase::memory(&[]).unwrap();
        db.put("session", "state", b"original").unwrap();
        let result: Result<(), String> = db.transaction(|| {
            db.remove("session", "state")?;
            Err("simulated import failure".to_owned())
        });
        assert!(result.is_err());
        assert_eq!(db.read("session", "state").unwrap().unwrap(), b"original");
        assert!(ContentDatabase::memory(b"not a database").is_err());
    }
}

#[cfg(test)]
mod relational_tests;
