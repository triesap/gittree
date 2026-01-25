ALTER TABLE relay_compatibility
    ADD COLUMN nip11_url TEXT,
    ADD COLUMN nip11_available BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN active_probe_ok BOOLEAN,
    ADD COLUMN active_probe_error TEXT;
