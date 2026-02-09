CREATE TABLE relay_tenant (
    id text PRIMARY KEY,
    host text NOT NULL UNIQUE,
    relay_pubkey bytea NOT NULL,
    relay_secret bytea NOT NULL,
    relay_secret_nonce bytea NOT NULL,
    relay_secret_kid text NOT NULL,
    name text,
    description text,
    icon text,
    banner text,
    contact text,
    auth_required boolean NOT NULL DEFAULT true,
    public_read boolean NOT NULL DEFAULT false,
    public_write boolean NOT NULL DEFAULT false,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);

CREATE INDEX relay_tenant_host_idx ON relay_tenant (host);
