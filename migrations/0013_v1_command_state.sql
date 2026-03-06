CREATE TABLE IF NOT EXISTS v1_command_log (
    event_id BYTEA PRIMARY KEY,
    pubkey BYTEA NOT NULL,
    namespace TEXT NOT NULL,
    action TEXT NOT NULL,
    target TEXT,
    args_json JSONB NOT NULL,
    status TEXT NOT NULL,
    code TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS v1_account_state (
    pubkey BYTEA PRIMARY KEY,
    status TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    deleted_at TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS v1_profile_state (
    pubkey BYTEA PRIMARY KEY,
    display_name TEXT,
    bio TEXT,
    avatar_url TEXT,
    website_url TEXT,
    location TEXT,
    visibility TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS v1_repo_state (
    owner_pubkey BYTEA NOT NULL,
    repo_name TEXT NOT NULL,
    description TEXT,
    website_url TEXT,
    visibility TEXT NOT NULL,
    default_branch TEXT NOT NULL,
    archived BOOLEAN NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (owner_pubkey, repo_name)
);

CREATE TABLE IF NOT EXISTS v1_repo_maintainer (
    owner_pubkey BYTEA NOT NULL,
    repo_name TEXT NOT NULL,
    maintainer_pubkey BYTEA NOT NULL,
    active BOOLEAN NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (owner_pubkey, repo_name, maintainer_pubkey)
);

CREATE INDEX IF NOT EXISTS idx_v1_command_log_pubkey_created_at
    ON v1_command_log (pubkey, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_v1_repo_maintainer_lookup
    ON v1_repo_maintainer (owner_pubkey, repo_name, active);
