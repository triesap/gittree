CREATE TABLE nostr_event (
    id bytea PRIMARY KEY,
    pubkey bytea NOT NULL,
    created_at bigint NOT NULL,
    kind integer NOT NULL,
    content text NOT NULL,
    sig bytea NOT NULL
);

CREATE INDEX nostr_event_created_at_idx ON nostr_event (created_at DESC, kind);
CREATE INDEX nostr_event_pubkey_idx ON nostr_event (pubkey);

CREATE TABLE nostr_tag (
    id bigserial PRIMARY KEY,
    event_id bytea NOT NULL REFERENCES nostr_event(id) ON DELETE CASCADE,
    name text NOT NULL,
    value text NOT NULL
);

CREATE INDEX nostr_tag_event_idx ON nostr_tag (event_id, name);
CREATE INDEX nostr_tag_value_idx ON nostr_tag (name, value);
