-- One row per distinct story (same link on HN and Lobsters = one row).
CREATE TABLE stories (
    key               TEXT PRIMARY KEY,
    title             TEXT NOT NULL,
    url               TEXT,
    self_text         TEXT,
    tags              TEXT NOT NULL DEFAULT '[]',
    first_source      TEXT NOT NULL,
    hn_id             TEXT,
    hn_score          INTEGER,
    hn_comments       INTEGER,
    lobsters_id       TEXT,
    lobsters_url      TEXT,
    lobsters_score    INTEGER,
    lobsters_comments INTEGER,
    -- pending: waiting to be posted; posted; skipped: seen on the first run
    -- and deliberately not posted; failed: gave up after repeated errors.
    status            TEXT NOT NULL,
    attempts          INTEGER NOT NULL DEFAULT 0,
    last_error        TEXT,
    -- LLM output, kept so a failed send or a later edit does not re-run the LLM.
    post_json         TEXT,
    first_seen_at     TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

CREATE INDEX stories_status ON stories (status);

-- Where each story was posted. Keyed by chat so more destinations can be
-- added later without changing the stories table.
CREATE TABLE posts (
    story_key  TEXT NOT NULL REFERENCES stories (key),
    chat_id    TEXT NOT NULL,
    message_id INTEGER NOT NULL,
    posted_at  TEXT NOT NULL,
    PRIMARY KEY (story_key, chat_id)
);
