# Todo API Example

A RESTful Todo API demonstrating database integration with axa.

## Features Demonstrated

- Database integration with `axa::db`
- SQLx for async PostgreSQL
- CRUD operations with proper error handling
- Request validation with validator crate
- UUID-based IDs
- Timestamps with chrono
- Structured JSON responses
- Database migrations

## Quick Start

### Prerequisites

- PostgreSQL 15+ (or use podman-compose from project root)
- Rust 1.97+ (or the rust-version in project's Cargo.toml)

### Configuration

Copy the example environment file:

```bash
cp .env.example .env
```

Edit `.env` with your database credentials:

```env
DATABASE_URL=postgres://axa:axa@localhost:5432/todo_api
PORT=3000
```

Or use the podman-compose setup from the project root:

```bash
# From project root
podman-compose -f podman-compose.yml up -d
# Database will be available at postgres://axa:axa@localhost:5432/axa_test
```

### Run the API

```bash
cd examples/todo-api
cargo run
```

### Test the API

```bash
# Create a todo
curl -X POST http://localhost:3000/todos \
  -H "Content-Type: application/json" \
  -d '{"title": "Buy groceries", "description": "Milk, eggs, bread"}'

# List all todos
curl http://localhost:3000/todos

# Get a specific todo
curl http://localhost:3000/todos/<uuid>

# Update a todo
curl -X PUT http://localhost:3000/todos/<uuid> \
  -H "Content-Type: application/json" \
  -d '{"completed": true}'

# Delete a todo
curl -X DELETE http://localhost:3000/todos/<uuid>

# Health check
curl http://localhost:3000/health
```

## API Endpoints

| Method | Path | Description | Request Body | Response |
|--------|------|-------------|--------------|----------|
| GET | /health | Health check | - | Service status |
| GET | /todos | List all todos | - | `TodoListResponse` |
| POST | /todos | Create a todo | `CreateTodoRequest` | `TodoResponse` |
| GET | /todos/:id | Get a todo | - | `TodoResponse` |
| PUT | /todos/:id | Update a todo | `UpdateTodoRequest` | `TodoResponse` |
| DELETE | /todos/:id | Delete a todo | - | `{status, id}` |

## Request/Response Types

### CreateTodoRequest

```json
{
  "title": "string (required, 1-255 chars)",
  "description": "string (optional)"
}
```

### UpdateTodoRequest

```json
{
  "title": "string (optional, 1-255 chars)",
  "description": "string (optional)",
  "completed": "boolean (optional)"
}
```

### TodoResponse

```json
{
  "id": "uuid",
  "title": "string",
  "description": "string or null",
  "completed": "boolean",
  "created_at": "RFC3339 timestamp",
  "updated_at": "RFC3339 timestamp"
}
```

### TodoListResponse

```json
{
  "todos": [TodoResponse],
  "total": 0
}
```

## Project Structure

```
todo-api/
├── Cargo.toml              # Dependencies
├── migrations/
│   └── 20261111150000_create_todos.sql  # Database migration
├── src/
│   └── main.rs             # Application entry point
├── .env.example            # Example environment variables
└── README.md               # This file
```

## Error Handling

The API returns standardized error responses:

- **400 Bad Request**: Validation errors
- **404 Not Found**: Resource not found
- **500 Internal Server Error**: Server errors

All errors follow the RFC 9457 Problem Details format when possible.

## Testing

```bash
# Run tests
cargo test

# With coverage
cargo tarpaulin
```

## Using with podman-compose

From the project root:

```bash
# Start PostgreSQL and Redis
podman-compose -f podman-compose.yml up -d

# Create the todo_api database
podman exec -it axa-postgres-1 psql -U axa -c "CREATE DATABASE todo_api;"

# Run the API
cd examples/todo-api
DATABASE_URL=postgres://axa:axa@localhost:5432/todo_api cargo run
```

## Next Steps

- [Basic Service Example](../basic-service) - Start here if you're new to axa
- [Multi-tenant Example](../multi-tenant) - See how to handle multiple tenants
- [axa Database Documentation](../../docs/database) - Learn about axa's database features
