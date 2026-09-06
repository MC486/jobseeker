-- Short-lived codes the CLI prints and the extension exchanges for a device token.
-- The plaintext code is never stored; only its BLAKE3 hash is.
CREATE TABLE pairing_code (
    id          TEXT PRIMARY KEY,
    code_hash   TEXT NOT NULL UNIQUE,
    name        TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    used_at     TEXT
) STRICT;

CREATE INDEX idx_pairing_code_expires ON pairing_code(expires_at);
