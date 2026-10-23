-- Lethetic relational schema v2. Shared by native and WASM hosts.
-- records is only an import/export identity catalog, never a session payload.
CREATE TABLE records (
    id INTEGER PRIMARY KEY,
    scope TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    UNIQUE(scope, name)
);
CREATE TABLE sessions (
    record_id INTEGER PRIMARY KEY REFERENCES records(id) ON DELETE CASCADE,
    session_id TEXT,
    display_name TEXT,
    label TEXT,
    updated_at TEXT,
    schema_version INTEGER,
    system_prompt TEXT,
    theme TEXT,
    connection_id TEXT,
    model_name TEXT,
    mode TEXT,
    python INTEGER,
    network_access INTEGER,
    nuget_access INTEGER,
    site_access INTEGER,
    relay_endpoint TEXT,
    hide_thinking INTEGER,
    loop_mode TEXT,
    remote_control TEXT,
    remote_control_open INTEGER,
    remote_control_files INTEGER,
    python_runtime_id TEXT,
    latest_logical_turn_id TEXT,
    context_tokens INTEGER,
    context_source TEXT,
    model_ms INTEGER,
    tool_ms INTEGER,
    elapsed_ms INTEGER,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL
);
CREATE TABLE messages (
    id INTEGER PRIMARY KEY,
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    turn_index INTEGER,
    role TEXT,
    content TEXT,
    tool_result_is_error INTEGER,
    tool_call_id TEXT,
    result_call_id TEXT,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL,
    FOREIGN KEY(record_id,turn_index) REFERENCES turns(record_id,turn_index) ON DELETE CASCADE
);
CREATE UNIQUE INDEX messages_position ON messages(record_id, position);
CREATE TABLE tool_calls (
    id INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    call_id TEXT,
    provider_id TEXT,
    function_name TEXT,
    arguments_json TEXT,
    call_type TEXT,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL
);
CREATE UNIQUE INDEX tool_calls_position ON tool_calls(message_id, position);
CREATE TABLE message_parts (
    id INTEGER PRIMARY KEY,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    part_type TEXT,
    text TEXT,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL
);
CREATE UNIQUE INDEX message_parts_position ON message_parts(message_id, position);
CREATE TABLE transcript_blocks (
    id INTEGER PRIMARY KEY,
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    kind TEXT,
    content TEXT,
    title TEXT,
    success INTEGER,
    logical_turn_id TEXT,
    duration_ms INTEGER,
    duration_label TEXT,
    prompt_tokens INTEGER,
    completion_tokens INTEGER,
    tool_name TEXT,
    tool_kind TEXT,
    tool_payload TEXT,
    uncached_input_tokens INTEGER,
    cache_read_input_tokens INTEGER,
    cache_creation_input_tokens INTEGER,
    output_tokens INTEGER,
    total_input_tokens INTEGER,
    total_tokens INTEGER,
    breakdown_complete INTEGER,
    reported_cost_nanos INTEGER,
    cost_currency TEXT,
    cost_nanos INTEGER,
    cost_incomplete INTEGER,
    cost_mixed_pricing INTEGER,
    cost_long_context INTEGER,
    cost_effective_as_of TEXT,
    cost_valid_through TEXT,
    cost_provenance TEXT,
    cost_display TEXT,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL
);
CREATE UNIQUE INDEX transcript_blocks_position ON transcript_blocks(record_id, position);
CREATE TABLE provider_requests (
    id INTEGER PRIMARY KEY,
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    request_id TEXT,
    logical_turn_id TEXT,
    connection_id TEXT,
    model TEXT,
    completed INTEGER,
    in_flight INTEGER,
    uncached_input_tokens INTEGER,
    cache_read_input_tokens INTEGER,
    cache_creation_input_tokens INTEGER,
    output_tokens INTEGER,
    total_input_tokens INTEGER,
    total_tokens INTEGER,
    breakdown_complete INTEGER,
    reported_cost_nanos INTEGER,
    cost_currency TEXT,
    cost_nanos INTEGER,
    cost_incomplete INTEGER,
    cost_mixed_pricing INTEGER,
    cost_long_context INTEGER,
    cost_effective_as_of TEXT,
    cost_valid_through TEXT,
    cost_provenance TEXT,
    cost_display TEXT,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL
);
CREATE UNIQUE INDEX provider_requests_position ON provider_requests(record_id, position);
CREATE TABLE usage_totals (
    id INTEGER PRIMARY KEY,
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    category TEXT NOT NULL,
    request_count INTEGER,
    long_context_request_count INTEGER,
    unpriced_request_count INTEGER,
    incomplete_usage_request_count INTEGER,
    responses INTEGER,
    input_reports INTEGER,
    output_reports INTEGER,
    total_reports INTEGER,
    cached_reports INTEGER,
    cache_write_reports INTEGER,
    reasoning_reports INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    total_tokens INTEGER,
    cached_tokens INTEGER,
    cache_write_tokens INTEGER,
    reasoning_tokens INTEGER,
    uncached_input_tokens INTEGER,
    cache_read_input_tokens INTEGER,
    cache_creation_input_tokens INTEGER,
    total_input_tokens INTEGER,
    breakdown_complete INTEGER,
    reported_cost_nanos INTEGER,
    cost_currency TEXT,
    cost_nanos INTEGER,
    cost_incomplete INTEGER,
    cost_mixed_pricing INTEGER,
    cost_long_context INTEGER,
    cost_effective_as_of TEXT,
    cost_valid_through TEXT,
    cost_provenance TEXT,
    cost_display TEXT,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL
);
CREATE TABLE connections (
    record_id INTEGER PRIMARY KEY REFERENCES records(id) ON DELETE CASCADE,
    name TEXT,
    provider TEXT,
    endpoint TEXT,
    model TEXT,
    thinking TEXT,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL
);
CREATE TABLE input_history (
    id INTEGER PRIMARY KEY,
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    content TEXT NOT NULL
);
CREATE UNIQUE INDEX input_history_position ON input_history(record_id, position);
CREATE TABLE files (
    record_id INTEGER PRIMARY KEY REFERENCES records(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    content BLOB NOT NULL
);
CREATE TABLE file_entries (
    id INTEGER PRIMARY KEY,
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    path TEXT,
    modified_at INTEGER,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL
);
CREATE UNIQUE INDEX file_entries_position ON file_entries(record_id, position);
CREATE TABLE directories (
    id INTEGER PRIMARY KEY,
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    path TEXT NOT NULL
);
CREATE UNIQUE INDEX directories_position ON directories(record_id, position);
CREATE TABLE todo_lists (
    record_id INTEGER PRIMARY KEY REFERENCES records(id) ON DELETE CASCADE,
    revision INTEGER,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL
);
CREATE TABLE todos (
    id INTEGER PRIMARY KEY,
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    todo_id TEXT,
    content TEXT,
    status TEXT,
    priority TEXT,
    fields_json TEXT NOT NULL,
    extra_json TEXT NOT NULL
);
CREATE UNIQUE INDEX todos_position ON todos(record_id, position);
CREATE UNIQUE INDEX usage_totals_category ON usage_totals(record_id, category);
CREATE INDEX messages_role ON messages(role);
CREATE INDEX tool_calls_name ON tool_calls(function_name);
CREATE INDEX provider_requests_turn ON provider_requests(record_id, logical_turn_id);
CREATE INDEX transcript_blocks_turn ON transcript_blocks(record_id, logical_turn_id);
CREATE TABLE turns (
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    turn_index INTEGER NOT NULL,
    first_message_position INTEGER NOT NULL,
    PRIMARY KEY(record_id, turn_index)
);
CREATE TABLE text_chunks (
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    content TEXT NOT NULL,
    PRIMARY KEY(record_id, ordinal)
);
CREATE TABLE binary_chunks (
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    content BLOB NOT NULL,
    PRIMARY KEY(record_id, ordinal)
);
CREATE INDEX messages_result_call ON messages(record_id, result_call_id);
CREATE TABLE tool_statistics (
    record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
    tool_name TEXT NOT NULL,
    call_count TEXT NOT NULL,
    PRIMARY KEY(record_id,tool_name)
);
CREATE VIEW logical_turn_usage AS
 SELECT record_id, logical_turn_id, cost_currency AS currency, COUNT(*) AS request_count,
 SUM(COALESCE(total_input_tokens,uncached_input_tokens+cache_read_input_tokens+cache_creation_input_tokens)) AS input_tokens,
 SUM(output_tokens) AS output_tokens, SUM(cost_nanos) AS cost_nanos
 FROM provider_requests GROUP BY record_id, logical_turn_id, cost_currency;
CREATE VIEW session_files AS
 SELECT s.session_id,e.path,e.modified_at,length(f.content) AS size_bytes,f.content
 FROM sessions s JOIN file_entries e ON e.record_id=s.record_id
 JOIN records r ON r.scope='browser/files/'||s.session_id AND r.name=e.path
 JOIN files f ON f.record_id=r.id;
