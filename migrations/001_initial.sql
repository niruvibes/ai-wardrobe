BEGIN IMMEDIATE;
CREATE TABLE garments (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, category TEXT NOT NULL,
    availability TEXT NOT NULL, favourite INTEGER NOT NULL CHECK(favourite IN (0,1)),
    archived INTEGER NOT NULL CHECK(archived IN (0,1)), data TEXT NOT NULL CHECK(json_valid(data))
);
CREATE INDEX garments_filter ON garments(archived,category,availability,favourite);
CREATE VIRTUAL TABLE garment_search USING fts5(item_id UNINDEXED, text, tokenize='unicode61');
CREATE TABLE profile_revisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT, profile TEXT NOT NULL CHECK(json_valid(profile)),
    preferences TEXT NOT NULL CHECK(json_valid(preferences)), created_at TEXT NOT NULL
);
CREATE TABLE affinity_revisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT, item_id TEXT NOT NULL REFERENCES garments(id),
    value REAL NOT NULL CHECK(value BETWEEN 0 AND 5), created_at TEXT NOT NULL
);
CREATE INDEX affinity_item ON affinity_revisions(item_id,id DESC);
CREATE TABLE outfits (signature TEXT PRIMARY KEY, data TEXT NOT NULL CHECK(json_valid(data)), favourite INTEGER NOT NULL CHECK(favourite IN (0,1)));
CREATE TABLE wear_events (id TEXT PRIMARY KEY, worn_at TEXT NOT NULL, data TEXT NOT NULL CHECK(json_valid(data)));
CREATE INDEX wear_date ON wear_events(worn_at DESC);
CREATE TABLE feedback_events (id TEXT PRIMARY KEY, signature TEXT NOT NULL, data TEXT NOT NULL CHECK(json_valid(data)));
CREATE INDEX feedback_signature ON feedback_events(signature);
CREATE TABLE settings (key TEXT PRIMARY KEY, data TEXT NOT NULL CHECK(json_valid(data)));
PRAGMA user_version=1;
COMMIT;
