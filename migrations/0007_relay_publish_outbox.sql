CREATE TABLE relay_publish_outbox (
    id bigserial PRIMARY KEY,
    relay_url text NOT NULL,
    event_id bytea NOT NULL,
    pubkey bytea NOT NULL,
    created_at bigint NOT NULL,
    kind integer NOT NULL,
    tags jsonb NOT NULL,
    content text NOT NULL,
    sig bytea NOT NULL,
    forgejo_owner text NOT NULL,
    forgejo_repo text NOT NULL,
    identifier text NOT NULL,
    status text NOT NULL,
    attempt_count integer NOT NULL DEFAULT 0,
    last_error text,
    publish_after timestamptz NOT NULL DEFAULT now(),
    created_at_ts timestamptz NOT NULL DEFAULT now(),
    updated_at_ts timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT relay_publish_outbox_status_check CHECK (
        status IN ('pending', 'publishing', 'published')
    )
);

CREATE INDEX relay_publish_outbox_status_idx
    ON relay_publish_outbox (status, publish_after, id);
CREATE INDEX relay_publish_outbox_repo_idx
    ON relay_publish_outbox (pubkey, identifier, kind);
