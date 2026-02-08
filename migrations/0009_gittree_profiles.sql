CREATE TABLE gittree_profile (
    pubkey BYTEA PRIMARY KEY,
    display_name TEXT,
    bio TEXT,
    avatar_url TEXT,
    website_url TEXT,
    location TEXT,
    visibility TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT gittree_profile_visibility CHECK (visibility IN ('private', 'public')),
    CONSTRAINT gittree_profile_updated CHECK (updated_at >= created_at)
);
