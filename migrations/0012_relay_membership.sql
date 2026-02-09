CREATE TABLE relay_membership (
    tenant_id text NOT NULL,
    pubkey bytea NOT NULL,
    role text NOT NULL,
    status text NOT NULL,
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL,
    PRIMARY KEY (tenant_id, pubkey)
);

CREATE INDEX relay_membership_tenant_idx ON relay_membership (tenant_id, status);

CREATE TABLE relay_invite (
    id bigserial PRIMARY KEY,
    tenant_id text NOT NULL,
    invite_code text NOT NULL UNIQUE,
    role text NOT NULL,
    inviter_pubkey bytea NOT NULL,
    invitee_pubkey bytea,
    expires_at bigint,
    created_at bigint NOT NULL
);

CREATE INDEX relay_invite_tenant_idx ON relay_invite (tenant_id);
