-- Every row names its tenant, and every query the service writes filters on it.
CREATE TABLE todos (
    tenant_id TEXT NOT NULL CHECK (tenant_id ~ '^[a-z0-9][a-z0-9-]{0,62}$'),
    id BIGINT GENERATED ALWAYS AS IDENTITY,
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200),
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id)
);

-- A second fence, in the database: a row is visible only to a transaction that set
-- app.tenant to its tenant. FORCE applies it to the table's owner too. A superuser, and
-- a role with BYPASSRLS, still sees every row, so the service connects as neither.
ALTER TABLE todos ENABLE ROW LEVEL SECURITY;
ALTER TABLE todos FORCE ROW LEVEL SECURITY;
CREATE POLICY todos_by_tenant ON todos
    USING (tenant_id = current_setting('app.tenant', true))
    WITH CHECK (tenant_id = current_setting('app.tenant', true));
