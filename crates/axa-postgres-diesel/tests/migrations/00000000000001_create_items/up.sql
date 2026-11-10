CREATE TYPE item_state AS ENUM ('new', 'done');

CREATE TABLE items (
    id BIGSERIAL PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    state item_state NOT NULL DEFAULT 'new'
);
