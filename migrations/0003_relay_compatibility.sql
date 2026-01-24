CREATE TABLE relay_compatibility (
    relay_url TEXT PRIMARY KEY,
    compatible BOOLEAN NOT NULL,
    supported_capabilities TEXT[] NOT NULL,
    missing_required TEXT[] NOT NULL,
    missing_optional TEXT[] NOT NULL,
    report JSONB NOT NULL,
    checked_at TIMESTAMPTZ NOT NULL
);
