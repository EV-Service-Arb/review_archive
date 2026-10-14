-- Additive only: a new table, so the previous release keeps working on this schema.

-- The Google accounts of `browser.profile_dirs`, by profile path. Only a person clears
-- signed_out_at, when they sign the profile in again (scripts/new_google_session.py).
CREATE TABLE profiles (
    path TEXT PRIMARY KEY,
    signed_out_at TEXT,
    used_at TEXT
);
