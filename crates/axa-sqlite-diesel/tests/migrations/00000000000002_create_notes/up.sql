CREATE TABLE notes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id INTEGER NOT NULL REFERENCES items (id),
    body TEXT NOT NULL
);

CREATE VIEW item_notes AS
    SELECT items.name, notes.body FROM items JOIN notes ON notes.item_id = items.id;
