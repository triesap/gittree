CREATE TABLE gittree_account (
    pubkey BYTEA PRIMARY KEY,
    forgejo_username TEXT NOT NULL UNIQUE
);
