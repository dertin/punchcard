CREATE TABLE projects (
    id TEXT PRIMARY KEY,
    root_path TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE change_intents (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    card_kind TEXT NOT NULL,
    memory_kind TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN (
            'candidate', 'in_progress', 'active', 'failed', 'incomplete',
            'stale', 'superseded', 'invalidated', 'historical'
        )
    ),
    required_validations_json TEXT NOT NULL,
    supersedes_card_id TEXT REFERENCES cards(id),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE cards (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    card_kind TEXT NOT NULL,
    memory_kind TEXT NOT NULL,
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    status TEXT NOT NULL CHECK (
        status IN (
            'candidate', 'in_progress', 'active', 'failed', 'incomplete',
            'stale', 'superseded', 'invalidated', 'historical'
        )
    ),
    source_refs_json TEXT NOT NULL,
    evidence_refs_json TEXT NOT NULL,
    valid_from TEXT,
    valid_until TEXT,
    supersedes_card_id TEXT REFERENCES cards(id),
    associated_files_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX one_active_card_per_supersession_chain
ON cards(project_id, supersedes_card_id)
WHERE status = 'active' AND supersedes_card_id IS NOT NULL;

CREATE TABLE attempts (
    id TEXT PRIMARY KEY,
    change_id TEXT NOT NULL REFERENCES change_intents(id),
    status TEXT NOT NULL CHECK (status IN ('in_progress', 'failed', 'incomplete', 'active')),
    summary TEXT,
    started_at TEXT NOT NULL,
    completed_at TEXT
);

CREATE TABLE validations (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    change_id TEXT NOT NULL REFERENCES change_intents(id),
    name TEXT NOT NULL,
    validation_level TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('passed', 'failed', 'timed_out')),
    evidence_json TEXT NOT NULL,
    validated_at TEXT NOT NULL,
    UNIQUE(change_id, name, id)
);

CREATE INDEX validations_by_change ON validations(change_id, name, validated_at DESC);

CREATE TABLE memory_events (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    project_id TEXT NOT NULL REFERENCES projects(id),
    change_id TEXT REFERENCES change_intents(id),
    card_id TEXT REFERENCES cards(id),
    event_kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    actor TEXT NOT NULL,
    checksum TEXT NOT NULL
);

CREATE TRIGGER memory_events_no_update
BEFORE UPDATE ON memory_events
BEGIN
    SELECT RAISE(ABORT, 'memory_events is append-only');
END;

CREATE TRIGGER memory_events_no_delete
BEFORE DELETE ON memory_events
BEGIN
    SELECT RAISE(ABORT, 'memory_events is append-only');
END;

CREATE TABLE document_sources (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    path TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    authority TEXT NOT NULL,
    status TEXT NOT NULL,
    content_hash TEXT,
    source_revision TEXT,
    indexed_at TEXT,
    UNIQUE(project_id, path)
);

CREATE TABLE document_chunks (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES document_sources(id) ON DELETE CASCADE,
    project_id TEXT NOT NULL REFERENCES projects(id),
    source_path TEXT NOT NULL,
    source_kind TEXT NOT NULL,
    authority TEXT NOT NULL,
    status TEXT NOT NULL,
    title_path TEXT NOT NULL,
    line_start INTEGER NOT NULL,
    line_end INTEGER NOT NULL,
    content TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    source_revision TEXT,
    indexed_at TEXT NOT NULL
);

CREATE VIRTUAL TABLE document_chunks_fts USING fts5(
    chunk_id UNINDEXED,
    title_path,
    content,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER document_chunks_fts_insert
AFTER INSERT ON document_chunks
BEGIN
    INSERT INTO document_chunks_fts(chunk_id, title_path, content)
    VALUES (new.id, new.title_path, new.content);
END;

CREATE TRIGGER document_chunks_fts_update
AFTER UPDATE OF title_path, content ON document_chunks
BEGIN
    DELETE FROM document_chunks_fts WHERE chunk_id = old.id;
    INSERT INTO document_chunks_fts(chunk_id, title_path, content)
    VALUES (new.id, new.title_path, new.content);
END;

CREATE TRIGGER document_chunks_fts_delete
AFTER DELETE ON document_chunks
BEGIN
    DELETE FROM document_chunks_fts WHERE chunk_id = old.id;
END;

CREATE VIRTUAL TABLE cards_fts USING fts5(
    card_id UNINDEXED,
    title,
    summary,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER cards_fts_insert
AFTER INSERT ON cards
BEGIN
    INSERT INTO cards_fts(card_id, title, summary)
    VALUES (new.id, new.title, new.summary);
END;

CREATE TRIGGER cards_fts_update
AFTER UPDATE OF title, summary ON cards
BEGIN
    DELETE FROM cards_fts WHERE card_id = old.id;
    INSERT INTO cards_fts(card_id, title, summary)
    VALUES (new.id, new.title, new.summary);
END;

CREATE TRIGGER cards_fts_delete
AFTER DELETE ON cards
BEGIN
    DELETE FROM cards_fts WHERE card_id = old.id;
END;

CREATE TABLE audit_log (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id TEXT REFERENCES projects(id),
    operation TEXT NOT NULL,
    subject TEXT,
    metadata_json TEXT NOT NULL,
    occurred_at TEXT NOT NULL
);

-- Session/task working-memory layer.
--
-- Sessions and tasks scope ephemeral observations to one codebase. Observations
-- are working memory only; they never become trusted current knowledge without
-- governed promotion through change_intents/cards.

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    title TEXT,
    status TEXT NOT NULL CHECK (status IN ('open', 'closed')),
    started_at TEXT NOT NULL,
    ended_at TEXT
);

CREATE INDEX sessions_by_project ON sessions(project_id, status, started_at DESC);

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    session_id TEXT NOT NULL REFERENCES sessions(id),
    parent_task_id TEXT REFERENCES tasks(id),
    agent_label TEXT,
    title TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('open', 'closed')),
    opened_at TEXT NOT NULL,
    closed_at TEXT
);

CREATE INDEX tasks_by_session ON tasks(session_id, opened_at DESC);
CREATE INDEX tasks_by_parent ON tasks(parent_task_id);

CREATE TABLE task_observations (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(id),
    session_id TEXT NOT NULL REFERENCES sessions(id),
    task_id TEXT NOT NULL REFERENCES tasks(id),
    title TEXT NOT NULL,
    summary TEXT NOT NULL,
    observation_kind TEXT NOT NULL CHECK (
        observation_kind IN ('note', 'summary', 'discovery', 'blocker', 'handoff')
    ),
    created_at TEXT NOT NULL,
    expires_at TEXT
);

CREATE INDEX task_observations_by_task ON task_observations(task_id, created_at DESC);
CREATE INDEX task_observations_by_session ON task_observations(session_id, created_at DESC);

CREATE VIRTUAL TABLE task_observations_fts USING fts5(
    observation_id UNINDEXED,
    title,
    summary,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER task_observations_fts_insert
AFTER INSERT ON task_observations
BEGIN
    INSERT INTO task_observations_fts(observation_id, title, summary)
    VALUES (new.id, new.title, new.summary);
END;

CREATE TRIGGER task_observations_fts_update
AFTER UPDATE OF title, summary ON task_observations
BEGIN
    DELETE FROM task_observations_fts WHERE observation_id = old.id;
    INSERT INTO task_observations_fts(observation_id, title, summary)
    VALUES (new.id, new.title, new.summary);
END;

CREATE TRIGGER task_observations_fts_delete
AFTER DELETE ON task_observations
BEGIN
    DELETE FROM task_observations_fts WHERE observation_id = old.id;
END;

