# Multi-tenant Service Example

A multi-tenant service demonstrating tenant isolation patterns with axa.

## Features Demonstrated

- **Schema-per-tenant isolation**: Each tenant gets a dedicated PostgreSQL schema
- **Tenant identification**: Extract tenant ID from `X-Tenant-ID` header
- **Tenant context**: Propagate tenant context through request lifecycle
- **Automatic schema provisioning**: Create schemas on-demand
- **Data isolation**: Tenants can only access their own data
- **Shared infrastructure**: Common tables in a shared schema

## Multi-tenancy Strategy

This example uses **schema-per-tenant** multi-tenancy:

```
PostgreSQL Database
├── public (default schema)
├── shared (shared tables across all tenants)
│   └── tenants (tenant metadata)
└── tenant_<tenant_id> (per-tenant schemas)
    └── todos (tenant-specific data)
```

### Pros of Schema-per-tenant:
- ✅ Strong data isolation
- ✅ Easy to backup/restore individual tenants
- ✅ Clear performance characteristics
- ✅ Can set per-schema permissions
- ✅ Easy to move tenants between servers

### Cons of Schema-per-tenant:
- ❌ More schemas to manage
- ❌ Connection limits per database
- ❌ Cross-tenant queries require multiple connections

## Quick Start

### Prerequisites

- PostgreSQL 15+
- Rust 1.97+

### Configuration

Copy the example environment file:

```bash
cp .env.example .env
```

Edit `.env` with your database credentials:

```env
DATABASE_URL=postgres://axa:axa@localhost:5432/multi_tenant
PORT=3000
```

### Run the Service

```bash
cd examples/multi-tenant
cargo run
```

### Test with Multiple Tenants

Open two terminal windows and test with different tenant IDs:

**Terminal 1 (Tenant A):**
```bash
# Create a todo for tenant A
curl -X POST http://localhost:3000/todos \
  -H "X-Tenant-ID: acme-corp" \
  -H "Content-Type: application/json" \
  -d '{"title": "Deploy to production", "description": "Deploy the new version"}'

# List todos for tenant A
curl -H "X-Tenant-ID: acme-corp" http://localhost:3000/todos
```

**Terminal 2 (Tenant B):**
```bash
# Create a todo for tenant B
curl -X POST http://localhost:3000/todos \
  -H "X-Tenant-ID: globex" \
  -H "Content-Type: application/json" \
  -d '{"title": "Review analytics", "description": "Check the quarterly reports"}'

# List todos for tenant B
curl -H "X-Tenant-ID: globex" http://localhost:3000/todos
```

**Verify Isolation:**
```bash
# Tenant A should only see their own todos
curl -H "X-Tenant-ID: acme-corp" http://localhost:3000/todos

# Tenant B should only see their own todos
curl -H "X-Tenant-ID: globex" http://localhost:3000/todos

# Health check (no tenant header required)
curl http://localhost:3000/health
```

## API Endpoints

| Method | Path | Description | Required Header |
|--------|------|-------------|------------------|
| GET | /health | Health check | None |
| GET | /todos | List todos for tenant | X-Tenant-ID |
| POST | /todos | Create a todo for tenant | X-Tenant-ID |
| GET | /todos/:id | Get a todo for tenant | X-Tenant-ID |
| PUT | /todos/:id | Update a todo for tenant | X-Tenant-ID |
| DELETE | /todos/:id | Delete a todo for tenant | X-Tenant-ID |

## Tenant Identification

### Current Implementation

The example uses a simple header-based approach:

```
X-Tenant-ID: <tenant-identifier>
```

The tenant identifier:
- Must be 1-64 characters
- Can contain letters, numbers, hyphens
- Is converted to lowercase with underscores for schema name
- Example: `acme-corp` → `tenant_acme_corp` schema

### Production Considerations

For production, consider:

1. **JWT Claims**: Extract tenant ID from a validated JWT token
2. **Subdomain**: Extract tenant from request host (tenant.app.com)
3. **API Key**: Use API keys with embedded tenant identifiers
4. **Database Lookup**: Validate tenant ID against a tenants table

Example JWT approach:

```rust
async fn jwt_tenant_middleware<B>(request: Request<B>, next: Next<B>) -> Result<Response, StatusCode> {
    let token = request.headers().get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    
    let claims: JwtClaims = decode_jwt(token)?;
    let tenant_id = claims.tenant_id;
    
    // Validate tenant exists in database
    validate_tenant(&tenant_id).await?;
    
    let ctx = TenantContext { tenant_id, schema_name: format!("tenant_{}", tenant_id) };
    let mut request = request;
    request.extensions_mut().insert(ctx);
    
    Ok(next.run(request).await)
}
```

## Database Schema Management

The example automatically creates tenant schemas on first access:

```sql
-- When a request comes in with X-Tenant-ID: acme-corp
-- The service creates:

CREATE SCHEMA tenant_acme_corp;

CREATE TABLE tenant_acme_corp.todos (
    id UUID PRIMARY KEY NOT NULL,
    title VARCHAR(255) NOT NULL,
    description TEXT,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    tenant_id VARCHAR(64) NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);

CREATE INDEX tenant_acme_corp.idx_todos_tenant_created 
    ON tenant_acme_corp.todos(tenant_id, created_at);
```

### Production Schema Management

For production, consider:

1. **Migration System**: Use a proper migration tool like sqlx or sea-orm
2. **Schema Validation**: Validate schemas exist before use
3. **Connection Pooling**: Consider separate connection pools per tenant
4. **Cleanup**: Automatically archive/drop unused tenant schemas

## Project Structure

```
multi-tenant/
├── Cargo.toml              # Dependencies
├── src/
│   └── main.rs             # Application entry point with multi-tenancy
├── .env.example            # Example environment variables
└── README.md               # This file
```

## Error Handling

The API returns standardized error responses:

- **400 Bad Request**: Missing or invalid tenant ID
- **404 Not Found**: Resource not found
- **500 Internal Server Error**: Database or server errors

## Testing

```bash
# Run tests
cargo test
```

## Performance Considerations

### Connection Pooling

For multi-tenant applications:
- Monitor connection pool metrics
- Consider larger pools if many tenants
- Use `PgBouncer` for connection pooling

### Query Performance

- Add indexes on tenant_id columns
- Monitor query performance per tenant
- Consider separate databases for very large tenants

### Schema Limits

- PostgreSQL has a limit of ~65,000 schemas per database
- Consider sharding databases if you exceed this limit
- Monitor schema creation performance

## Using with podman-compose

From the project root:

```bash
# Start PostgreSQL
podman-compose -f podman-compose.yml up -d postgres

# Create the multi_tenant database
podman exec -it axa-postgres-1 psql -U axa -c "CREATE DATABASE multi_tenant;"

# Run the service
cd examples/multi-tenant
DATABASE_URL=postgres://axa:axa@localhost:5432/multi_tenant cargo run
```

## Next Steps

- [Basic Service Example](../basic-service) - Start here if you're new to axa
- [Todo API Example](../todo-api) - See database integration without multi-tenancy
- [axa Multi-tenancy Documentation](../../docs/multi-tenancy) - Learn about axa's multi-tenancy support
