CREATE INDEX nostr_event_kind_created_at_idx ON nostr_event (kind, created_at DESC);
CREATE INDEX nostr_event_pubkey_kind_created_at_idx ON nostr_event (pubkey, kind, created_at DESC);
CREATE INDEX nostr_tag_name_value_event_idx ON nostr_tag (name, value, event_id);
