# API Documentation

This document describes the API endpoints for this service.

## Base URL

```
https://api.your-service.com/v1
```

## Authentication

All endpoints require authentication unless noted otherwise.

### Authentication Methods

| Method | Description | Header |
|--------|-------------|--------|
| None | Public endpoints | N/A |
| Bearer Token | JWT token | `Authorization: Bearer <token>` |

## Endpoints

### Health Checks

#### GET /_health/live

Liveness probe. Returns 200 if the service is running.

**Request**
```http
GET /_health/live
```

**Response**
```json
{
  "status": "live"
}
```

**Status Codes**
- 200 OK - Service is alive

---

#### GET /_health/ready

Readiness probe. Returns 200 if the service is ready to accept traffic.

**Request**
```http
GET /_health/ready
```

**Response**
```json
{
  "status": "ready"
}
```

**Status Codes**
- 200 OK - Service is ready
- 503 Service Unavailable - Service is not ready

---

#### GET /_health/startup

Startup probe. Returns 200 once the service has completed startup.

**Request**
```http
GET /_health/startup
```

**Response**
```json
{
  "status": "started",
  "version": "0.1.0",
  "service": "service-template"
}
```

**Status Codes**
- 200 OK - Service has started
- 503 Service Unavailable - Service is starting up

---

## Error Responses

All errors follow RFC 9457 (Problem Details) format.

### Standard Error Response

```json
{
  "type": "https://example.com/errors/<error-type>",
  "title": "Error Title",
  "status": 400,
  "detail": "Error description",
  "instance": "/api/endpoint"
}
```

### Error Types

| Type | Title | Status | Description |
|------|-------|--------|-------------|
| `https://example.com/errors/validation` | Validation Error | 400 | Request data is invalid |
| `https://example.com/errors/not-found` | Not Found | 404 | Resource doesn't exist |
| `https://example.com/errors/conflict` | Conflict | 409 | Resource state conflict |
| `https://example.com/errors/rate-limit` | Rate Limit Exceeded | 429 | Too many requests |
| `https://example.com/errors/internal` | Internal Server Error | 500 | Unexpected server error |
| `https://example.com/errors/unavailable` | Service Unavailable | 503 | Service not ready |

### Validation Errors

For validation errors, the response includes field-specific details:

```json
{
  "type": "https://example.com/errors/validation",
  "title": "Validation Error",
  "status": 400,
  "detail": "One or more fields are invalid",
  "errors": {
    "email": ["Invalid email format"],
    "password": ["Password must be at least 8 characters"]
  }
}
```

## Rate Limiting

### Rate Limit Headers

All responses include rate limit information:

| Header | Description |
|--------|-------------|
| `X-RateLimit-Limit` | Maximum requests allowed |
| `X-RateLimit-Remaining` | Remaining requests |
| `X-RateLimit-Reset` | Unix timestamp when limit resets |

### Rate Limit Response

When rate limit is exceeded:

```json
{
  "type": "https://example.com/errors/rate-limit",
  "title": "Rate Limit Exceeded",
  "status": 429,
  "detail": "Too many requests",
  "retry_after": 60
}
```

## Versioning

API versioning follows semantic versioning:

- **Major version (v1, v2, ...)**: Breaking changes
- **Minor version (v1.1, v1.2, ...)**: Backwards-compatible changes
- **Patch version (v1.1.1, v1.1.2, ...)**: Bug fixes

### Version Headers

| Header | Description |
|--------|-------------|
| `Accept: application/vnd.your-service.v1+json` | Request specific version |
| `Content-Type: application/vnd.your-service.v1+json` | Response version |

## Pagination

For endpoints that return lists of items, pagination is supported.

### Request Parameters

| Parameter | Type | Description | Default |
|-----------|------|-------------|---------|
| `page` | integer | Page number (1-based) | 1 |
| `per_page` | integer | Items per page | 20 |
| `cursor` | string | Opaque cursor for next page | - |

### Response Format

```json
{
  "data": [
    { "id": 1, "name": "Item 1" },
    { "id": 2, "name": "Item 2" }
  ],
  "pagination": {
    "page": 1,
    "per_page": 20,
    "total": 100,
    "total_pages": 5,
    "has_next": true,
    "has_previous": false,
    "next_cursor": "abc123...",
    "previous_cursor": null
  }
}
```

## Request Examples

### Using curl

```bash
# GET request
curl -X GET https://api.your-service.com/v1/resource \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json"

# POST request
curl -X POST https://api.your-service.com/v1/resource \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"name": "value"}'

# With pagination
curl -X GET "https://api.your-service.com/v1/resource?page=2&per_page=50" \
  -H "Authorization: Bearer <token>"
```

### Using JavaScript (fetch)

```javascript
const response = await fetch('https://api.your-service.com/v1/resource', {
  method: 'GET',
  headers: {
    'Authorization': 'Bearer <token>',
    'Content-Type': 'application/json'
  }
});

const data = await response.json();
```

### Using Python (requests)

```python
import requests

response = requests.get(
    'https://api.your-service.com/v1/resource',
    headers={
        'Authorization': 'Bearer <token>',
        'Content-Type': 'application/json'
    }
)

data = response.json()
```

## Response Examples

### Success Response

```json
{
  "data": {
    "id": 123,
    "name": "Example",
    "created_at": "2026-01-01T00:00:00Z",
    "updated_at": "2026-01-01T00:00:00Z"
  }
}
```

### List Response

```json
{
  "data": [
    { "id": 1, "name": "Item 1" },
    { "id": 2, "name": "Item 2" },
    { "id": 3, "name": "Item 3" }
  ],
  "pagination": {
    "page": 1,
    "per_page": 20,
    "total": 3,
    "total_pages": 1,
    "has_next": false,
    "has_previous": false
  }
}
```

### Empty Response

```json
{
  "data": null
}
```

## Webhooks

### Webhook Format

Webhooks are sent as POST requests with JSON body:

```json
{
  "event": "resource.created",
  "timestamp": "2026-01-01T00:00:00Z",
  "id": "event-123",
  "data": {
    "id": 123,
    "name": "Example"
  }
}
```

### Webhook Headers

| Header | Description |
|--------|-------------|
| `X-Webhook-Signature` | HMAC-SHA256 signature |
| `X-Webhook-Event` | Event type |
| `X-Webhook-Timestamp` | Event timestamp |

### Webhook Verification

```rust
use axa::webhooks::{verify_webhook, WebhookConfig};

let config = WebhookConfig {
    secret: "your-webhook-secret".to_string(),
    timestamp_tolerance: 300, // 5 minutes
};

let is_valid = verify_webhook(&request, &config).await;
```

## API Changelog

| Version | Date | Changes |
|---------|------|---------|
| v1.0.0 | 2026-11-XX | Initial API |

## Deprecation Policy

- Deprecated endpoints are marked with `Sunset` header
- Deprecated endpoints are removed after 1 year
- Breaking changes are only in major versions

## Support

For API support, please:
1. Check this documentation
2. Check the error message and type
3. Open an issue on GitHub

## License

This API is provided under the same license as the service.
