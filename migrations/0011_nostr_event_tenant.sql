ALTER TABLE nostr_event
    ADD COLUMN tenant_id text NOT NULL DEFAULT '';

ALTER TABLE nostr_tag
    ADD COLUMN tenant_id text NOT NULL DEFAULT '';

CREATE INDEX nostr_event_tenant_idx ON nostr_event (tenant_id, created_at DESC, kind);
CREATE INDEX nostr_tag_tenant_idx ON nostr_tag (tenant_id, name, value);
