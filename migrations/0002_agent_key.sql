-- An account's key for an agent, kept as the SHA-256 it is found by, like a session's token.
ALTER TABLE users ADD COLUMN key_hash bytea UNIQUE CHECK (octet_length(key_hash) = 32);
