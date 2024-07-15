CREATE TABLE feeds (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    last_updated INTEGER NOT NULL
);

CREATE TABLE entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    feed_id INTEGER NOT NULL REFERENCES feeds (id) ON DELETE CASCADE,
    retrieved INTEGER NOT NULL,
    entry_id TEXT NOT NULL,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    url TEXT NOT NULL,
    author TEXT,
    published INTEGER,

    UNIQUE (feed_id, entry_id)
);
