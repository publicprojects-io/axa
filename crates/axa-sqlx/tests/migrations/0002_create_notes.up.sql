CREATE TABLE notes (
    id BIGSERIAL PRIMARY KEY,
    item_id BIGINT NOT NULL REFERENCES items (id),
    body TEXT NOT NULL
);
