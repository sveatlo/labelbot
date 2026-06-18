CREATE TABLE IF NOT EXISTS processed_messages (
    message_id TEXT PRIMARY KEY NOT NULL,
    labels TEXT NOT NULL DEFAULT '',
    processed_at INTEGER NOT NULL
);
