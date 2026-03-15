# UGENT-LINE-PROXY RMS CLI & API Guide

**Relationship Management System (RMS)** - Manage LINE-to-UGENT client relationships via CLI and REST API.

---

## Table of Contents

1. [Overview](#overview)
2. [CLI Usage](#cli-usage)
3. [REST API Reference](#rest-api-reference)
4. [Examples](#examples)

---

## Overview

RMS allows you to:
- View connected UGENT LINE clients
- Manage LINE entities (users, groups, rooms)
- Configure which client handles which LINE conversation
- View dispatch rules (how messages are routed)
- Import/export relationships for backup

### Architecture

```
┌─────────────────┐     ┌─────────────────┐     ┌─────────────────┐
│  LINE Platform  │────▶│  ugent-line-proxy│────▶│  UGENT Clients  │
│  (Webhook)      │     │  (RMS)          │     │  (WebSocket)    │
└─────────────────┘     └─────────────────┘     └─────────────────┘
                              │
                    ┌─────────┴─────────┐
                    │                   │
               ┌────▼────┐        ┌─────▼─────┐
               │ REST API│        │    CLI    │
               │/api/rms │        │  rms-cli  │
               └─────────┘        └───────────┘
```

---

## CLI Usage

### Building the CLI

```bash
cd ugent-line-proxy
cargo build --bin rms-cli
```

The binary will be at `./target/debug/rms-cli`

### Global Options

```bash
rms-cli [COMMAND] [OPTIONS]
```

### Commands

#### Status

Show system status including connected clients and statistics.

```bash
rms-cli status
```

**Output Example:**
```
╭────────────────────────────────────╮
│ RMS Status                         │
├────────────────────────────────────┤
│ Connected Clients: 3               │
│ Total Entities: 25                 │
│ Relationships: 10                  │
│ Uptime: 2h 30m                     │
╰────────────────────────────────────╯
```

---

#### Clients

Manage connected UGENT LINE clients.

**List all clients:**
```bash
rms-cli clients list
```

**Show client details:**
```bash
rms-cli clients show <CLIENT_ID>
```

**Output Example:**
```
ID: client-001
Name: UGENT-Desktop
Connected: 2026-03-15 10:30:00
Last Activity: 2026-03-15 12:45:30
Assigned Entities: 5
```

---

#### Entities

Manage LINE entities (users, groups, rooms).

**List entities:**
```bash
# List all
rms-cli entities list

# Filter by type
rms-cli entities list --type user
rms-cli entities list --type group
rms-cli entities list --type room

# Filter by relationship status
rms-cli entities list --assigned     # Only with relationships
rms-cli entities list --unassigned   # Only without relationships

# Search by name
rms-cli entities list --search "John"
```

**Show entity details:**
```bash
rms-cli entities show <ENTITY_ID>
```

**Refresh entity from LINE API:**
```bash
rms-cli entities refresh <ENTITY_ID>
```

**Output Example:**
```
ID: U1234567890abcdef
Type: user
Display Name: John Doe
Picture URL: https://...
Relationship: client-001 (assigned)
Last Message: 2026-03-15 12:00:00
```

---

#### Relationships

Manage entity-to-client relationships (which client handles which entity).

**List all relationships:**
```bash
rms-cli relationships list
```

**Show relationship for an entity:**
```bash
rms-cli relationships show <ENTITY_ID>
```

**Set a relationship (assign entity to client):**
```bash
rms-cli relationships set <ENTITY_ID> <CLIENT_ID> [--notes "Optional notes"]
```

**Remove a relationship:**
```bash
rms-cli relationships remove <ENTITY_ID>
```

**Clear all manual relationships:**
```bash
rms-cli relationships clear
```

**Output Example:**
```
✓ Relationship created/updated

Entity: U1234567890abcdef (John Doe)
Client: client-001 (UGENT-Desktop)
Assigned: 2026-03-15 12:00:00
Notes: Primary work account
```

---

#### Rules

View dispatch rules (how messages are routed to clients).

**List all dispatch rules:**
```bash
rms-cli rules list
```

**Show rule for a conversation:**
```bash
rms-cli rules show <CONVERSATION_ID>
```

**Output Example:**
```
Conversation: C1234567890abcdef
Type: group
Dispatch To: client-001
Rule Source: manual_relationship
Priority: 100
```

---

#### Import/Export

**Export relationships (JSON to stdout):**
```bash
rms-cli export > relationships.json
```

**Import relationships:**
```bash
rms-cli import < relationships.json
```

---

#### Sync

Sync relationships with runtime ownership state.

```bash
rms-cli sync
```

This reconciles stored relationships with actual WebSocket client ownership.

---

## REST API Reference

Base URL: `http://localhost:PORT/api/rms`

All responses follow this format:

```json
{
  "success": true,
  "data": { ... }
}
```

Error responses:
```json
{
  "success": false,
  "error": "Error message"
}
```

---

### Status

**GET** `/api/rms/status`

Get system status.

**Response:**
```json
{
  "success": true,
  "data": {
    "connected_clients": 3,
    "total_entities": 25,
    "total_relationships": 10,
    "uptime_secs": 9000
  }
}
```

---

### Clients

**GET** `/api/rms/clients`

List all connected clients.

**Response:**
```json
{
  "success": true,
  "data": [
    {
      "id": "client-001",
      "name": "UGENT-Desktop",
      "connected_at": "2026-03-15T10:30:00Z",
      "last_activity": "2026-03-15T12:45:30Z"
    }
  ]
}
```

**GET** `/api/rms/clients/{id}`

Get client details.

---

### Entities

**GET** `/api/rms/entities`

List LINE entities.

**Query Parameters:**
| Parameter | Type | Description |
|-----------|------|-------------|
| `type` | string | Filter: `user`, `group`, `room` |
| `has_relationship` | bool | Filter by assignment status |
| `search` | string | Search by display name |
| `limit` | number | Max results (default: 100) |
| `offset` | number | Pagination offset |

**Example:**
```
GET /api/rms/entities?type=group&has_relationship=true&limit=50
```

**Response:**
```json
{
  "success": true,
  "data": {
    "items": [
      {
        "id": "C1234567890abcdef",
        "entity_type": "group",
        "display_name": "Work Team",
        "picture_url": "https://...",
        "has_relationship": true
      }
    ],
    "total": 10,
    "offset": 0,
    "limit": 50
  }
}
```

**GET** `/api/rms/entities/{id}`

Get entity details.

**POST** `/api/rms/entities/{id}/refresh`

Refresh entity from LINE API.

---

### Relationships

**GET** `/api/rms/relationships`

List all relationships.

**Response:**
```json
{
  "success": true,
  "data": [
    {
      "line_entity_id": "U1234567890abcdef",
      "client_id": "client-001",
      "assigned_at": "2026-03-15T12:00:00Z",
      "notes": "Primary work account"
    }
  ]
}
```

**GET** `/api/rms/relationships/{entity_id}`

Get relationship for an entity.

**POST** `/api/rms/relationships`

Create or update a relationship.

**Request Body:**
```json
{
  "entity_id": "U1234567890abcdef",
  "client_id": "client-001",
  "notes": "Optional notes"
}
```

**Response:**
```json
{
  "success": true,
  "data": {
    "line_entity_id": "U1234567890abcdef",
    "client_id": "client-001",
    "assigned_at": "2026-03-15T12:00:00Z",
    "notes": "Optional notes"
  }
}
```

**DELETE** `/api/rms/relationships/{entity_id}`

Remove a relationship.

**Response:**
```json
{
  "success": true,
  "data": {
    "removed": true
  }
}
```

---

### Dispatch Rules

**GET** `/api/rms/dispatch-rules`

List all dispatch rules.

**GET** `/api/rms/dispatch-rules/{conversation_id}`

Get dispatch rule for a conversation.

**Response:**
```json
{
  "success": true,
  "data": {
    "conversation_id": "C1234567890abcdef",
    "conversation_type": "group",
    "dispatch_to_client": "client-001",
    "rule_source": "manual_relationship",
    "priority": 100
  }
}
```

---

### Import/Export

**GET** `/api/rms/export`

Export all relationships as JSON.

**POST** `/api/rms/import`

Import relationships.

**Request Body:**
```json
{
  "relationships": [
    {
      "entity_id": "U1234567890abcdef",
      "client_id": "client-001",
      "notes": "Imported"
    }
  ]
}
```

---

### Maintenance

**POST** `/api/rms/sync`

Sync relationships with runtime ownership.

**POST** `/api/rms/clear`

Clear all manual relationships.

**Response:**
```json
{
  "success": true,
  "data": {
    "cleared_count": 10
  }
}
```

---

## Examples

### Assign a group to a specific client

**CLI:**
```bash
rms-cli relationships set C1234567890abcdef client-001 --notes "Work team group"
```

**API (curl):**
```bash
curl -X POST http://localhost:3000/api/rms/relationships \
  -H "Content-Type: application/json" \
  -d '{
    "entity_id": "C1234567890abcdef",
    "client_id": "client-001",
    "notes": "Work team group"
  }'
```

### Find unassigned groups

**CLI:**
```bash
rms-cli entities list --type group --unassigned
```

**API:**
```bash
curl "http://localhost:3000/api/rms/entities?type=group&has_relationship=false"
```

### Backup and restore relationships

**Backup:**
```bash
rms-cli export > backup_$(date +%Y%m%d).json
```

**Restore:**
```bash
rms-cli import < backup_20260315.json
```

### Check dispatch routing

**CLI:**
```bash
rms-cli rules show C1234567890abcdef
```

**API:**
```bash
curl http://localhost:3000/api/rms/dispatch-rules/C1234567890abcdef
```

---

## Configuration

The RMS uses the same SQLite database as the main proxy:

**Default database path:** `~/.ugent/line-plugin/line-proxy.db`

This can be configured in `config.toml`:

```toml
[storage]
database_path = "~/.ugent/line-plugin/line-proxy.db"
```

---

## Troubleshooting

### CLI not connecting

Ensure the proxy server is running:
```bash
cargo run --bin ugent-line-proxy
```

### No clients showing

Check WebSocket connections:
```bash
rms-cli status
```

Clients must connect via WebSocket to the proxy.

### Entity not found

Refresh the entity from LINE API:
```bash
rms-cli entities refresh <ENTITY_ID>
```

---

## See Also

- [RMS_IMPLEMENTATION_STATUS.md](./RMS_IMPLEMENTATION_STATUS.md) - Implementation details
- [LINE_PROXY_SERVER_GUIDE.md](./LINE_PROXY_SERVER_GUIDE.md) - Proxy server setup
- [LINE_CLIENT_PLUGIN_GUIDE.md](./LINE_CLIENT_PLUGIN_GUIDE.md) - Client plugin setup
