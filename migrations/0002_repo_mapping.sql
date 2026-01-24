CREATE TABLE repo_mapping (
    id BIGSERIAL PRIMARY KEY,
    forgejo_owner TEXT NOT NULL,
    forgejo_repo TEXT NOT NULL,
    pubkey BYTEA NOT NULL,
    identifier TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (forgejo_owner, forgejo_repo),
    UNIQUE (pubkey, identifier)
);
CREATE INDEX repo_mapping_lookup_idx
    ON repo_mapping (forgejo_owner, forgejo_repo);
CREATE INDEX repo_mapping_nostr_idx
    ON repo_mapping (pubkey, identifier);
