CREATE TABLE repo_announcement (
    id BIGSERIAL PRIMARY KEY,
    event_id BYTEA NOT NULL UNIQUE,
    pubkey BYTEA NOT NULL,
    identifier TEXT NOT NULL,
    name TEXT,
    description TEXT,
    root_commit TEXT,
    clone_urls TEXT[] NOT NULL,
    web_urls TEXT[] NOT NULL DEFAULT '{}',
    relays TEXT[] NOT NULL,
    blossoms TEXT[] NOT NULL DEFAULT '{}',
    hashtags TEXT[] NOT NULL DEFAULT '{}',
    maintainers TEXT[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX repo_announcement_lookup_idx
    ON repo_announcement (pubkey, identifier, created_at DESC);
CREATE TABLE repo_state (
    id BIGSERIAL PRIMARY KEY,
    event_id BYTEA NOT NULL UNIQUE,
    pubkey BYTEA NOT NULL,
    identifier TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    state JSONB NOT NULL
);
CREATE INDEX repo_state_lookup_idx
    ON repo_state (pubkey, identifier, created_at DESC);
