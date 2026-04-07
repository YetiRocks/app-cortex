<p align="center">
  <img src="https://cdn.prod.website-files.com/68e09cef90d613c94c3671c0/697e805a9246c7e090054706_logo_horizontal_grey.png" alt="Yeti" width="200" />
</p>

---

# app-cortex

[![Yeti](https://img.shields.io/badge/Yeti-Application-blue)](https://yetirocks.com)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

> **[Yeti](https://yetirocks.com)** - The Performance Platform for Agent-Driven Development.
> Schema-driven APIs, real-time streaming, and vector search. From prompt to production.

**The memory layer for AI agents.** Persistent, searchable, real-time, offline-first.

Cortex gives every AI agent in your stack shared long-term memory — semantic vector search, automatic classification, content deduplication, and real-time sync. No external API keys required. No separate infrastructure. No cloud account. One yeti application, zero configuration.

---

## Why Cortex

Every AI tool has amnesia. Switch from Claude to Cursor, and your agent forgets everything. Multi-agent systems need shared memory, but building that means wiring together a vector database, an embedding service, a classification pipeline, and a message broker — four services for one capability.

Cortex collapses all of that into a single yeti application:

- **Automatic local embeddings** — text is embedded on write and searched on query using built-in ONNX models. No API keys, no external services, no configuration. Works offline.
- **HNSW vector indexing** — 384-dimensional cosine similarity search built into the table layer. Sub-millisecond nearest-neighbor queries on native Rust indexes.
- **Content deduplication** — content-hash dedup on write prevents duplicate memories. Source-based dedup updates existing records instead of creating new ones.
- **Automatic classification** — keyword-based classification works offline with zero config. Optional LLM classification via Anthropic, OpenAI, or Ollama with graceful fallback.
- **Real-time propagation** — when one agent stores a memory, every subscriber gets it immediately via SSE or MQTT. Native to the platform, no external broker.
- **Project context ingestion** — ingest CLAUDE.md, .cursor/rules, .windsurf/ files and make them semantically searchable. Re-ingest detects changes, updates sections, and marks stale content as superseded.
- **Hybrid retrieval** — vector similarity for meaning, indexed fields for classification, source, agent, and channel. One query, both axes.
- **Single binary deployment** — compiles into a native Rust plugin. No Node.js, no npm, no Docker compose. Loads with yeti in seconds.

---

## Quick Start

### 1. Install

```bash
cd ~/yeti/applications
git clone https://github.com/yetirocks/app-cortex.git
```

Restart yeti. Cortex compiles automatically on first load (~2 minutes) and is cached for subsequent starts (~10 seconds).

### 2. Store a memory

```bash
curl -X POST https://localhost:9996/app-cortex/api/store \
  -H "Content-Type: application/json" \
  -d '{
    "content": "We decided to use RocksDB instead of LMDB for better write throughput",
    "source": "conversation",
    "agentId": "claude-1"
  }'
```

Response:
```json
{
  "id": "mem-1743292800-a1b2c3d4",
  "action": "created",
  "classification": "decision",
  "contentHash": "3f8a9c2b1d4e7f06"
}
```

The memory is automatically embedded via BAAI/bge-small-en-v1.5, classified as a "decision" (keyword match on "decided"), and indexed for vector search.

### 3. Search by meaning

```bash
curl "https://localhost:9996/app-cortex/api/Memory?query=embedding==vector:\"storage engine choice\"&limit=5"
```

Returns memories ranked by semantic similarity — not keyword matching. "Storage engine choice" finds the RocksDB decision even though neither word appears in the query.

### 4. Stream updates in real-time

```bash
# SSE stream — get notified when any agent stores a memory
curl "https://localhost:9996/app-cortex/api/Memory?stream=sse"

# MQTT — subscribe to memory changes
mosquitto_sub -t "app-cortex/Memory" -h localhost -p 8883
```

### 5. Ingest project context

```bash
curl -X POST https://localhost:9996/app-cortex/api/ingest \
  -H "Content-Type: application/json" \
  -d '{
    "projectId": "my-project",
    "source": ".claude/CLAUDE.md",
    "content": "# Project Rules\n## Testing\nAlways write integration tests.\n## Style\nUse snake_case for Rust functions."
  }'
```

Response:
```json
{
  "projectId": "my-project",
  "source": ".claude/CLAUDE.md",
  "sourceFormat": "claude-md",
  "chunks": 2,
  "inserted": 2,
  "updated": 0,
  "unchanged": 0
}
```

Each `##` section becomes a separate Synapse record with its own vector embedding. Re-ingesting the same file updates changed sections, skips unchanged ones, and marks removed sections as superseded.

### 6. Search project context

```bash
curl "https://localhost:9996/app-cortex/api/Synapse?query=embedding==vector:\"naming conventions\"&projectId==my-project&limit=5"
```

---

## Architecture

```
AI Agents (Claude Code, Cursor, Windsurf, ChatGPT, local models)
    |
    +-- MCP tools ---------> app-cortex (per-app MCP, auto-generated)
    +-- REST / SSE --------> app-cortex (schema-driven endpoints)
    +-- MQTT pub/sub ------> app-cortex (native broker)
          |
          v
    +------------------------------------------+
    |              app-cortex                   |
    |  +----------+  +----------+  +--------+  |
    |  |  Memory  |  |  Synapse |  |Settings|  |
    |  |  (HNSW)  |  |  (HNSW)  |  |       |  |
    |  +----------+  +----------+  +--------+  |
    |                                          |
    |  store -> dedup -> classify -> embed     |
    |  ingest -> chunk -> dedup -> embed       |
    |  classify -> keyword | LLM fallback      |
    +------------------------------------------+
          |
          v
    Yeti (embedded RocksDB, native HNSW, MQTT broker)
```

**Write path:** Agent request -> content sanitization (64KB limit) -> content-hash dedup -> keyword classification -> auto-embed via fastembed ONNX -> store in RocksDB with HNSW index -> broadcast via SSE + MQTT.

**Read path:** Natural language query -> auto-embed query text -> HNSW nearest-neighbor search -> ranked results with similarity scores. Combine with structured filters on any indexed field.

---

## Features

### Memory Storage (POST /app-cortex/api/store)

Smart memory storage with built-in deduplication:

| Field | Type | Description |
|-------|------|-------------|
| `content` | String (required) | The memory content (max 64KB) |
| `source` | String | Origin: "conversation", "slack", "manual", "tool" |
| `sourceId` | String | External reference for source-based dedup |
| `agentId` | String | Which agent stored this memory |
| `channelId` | String | Conversation/channel grouping |
| `metadata` | String (JSON) | Arbitrary key-value pairs |

**Deduplication logic:**
1. If `sourceId` provided: finds existing record with same `source` + `sourceId` and updates it (returns `"action": "updated"`)
2. Computes content hash: if identical content already exists, returns existing record (returns `"action": "duplicate"`)
3. Otherwise: inserts new record with auto-classification and auto-embedding (returns `"action": "created"`)

### Context Ingestion (POST /app-cortex/api/ingest)

Ingest project configuration files and make them semantically searchable:

| Field | Type | Description |
|-------|------|-------------|
| `projectId` | String (required) | Project identifier for scoped search |
| `source` | String (required) | File path or URL |
| `content` | String (required) | Raw file content (max 1MB) |
| `sourceFormat` | String | Auto-detected: "claude-md", "cursor-rules", "windsurf", "markdown", "custom" |
| `tags` | String (JSON array) | Searchable tags |

**Chunking:** Splits Markdown by `#` and `##` headings. Each section becomes a separate record with its own vector embedding. H2 sections track their parent H1 for hierarchical context.

**Re-ingestion:** Detects changes per section via content hash. Updates changed sections, skips unchanged ones, marks removed sections as `"superseded"`.

**Supported formats:**
| Format | Auto-detected from |
|--------|-------------------|
| CLAUDE.md | Paths containing "claude" ending in ".md" |
| Cursor rules | Paths containing ".cursor" or "cursor" |
| Windsurf config | Paths containing ".windsurf" or "windsurf" |
| Markdown | Any `.md` file |
| Custom | Everything else |

### Classification (POST /app-cortex/api/classify)

Classify or reclassify memories using keyword rules or external LLMs:

| Mode | Request body |
|------|-------------|
| Single memory | `{ "id": "mem-123" }` |
| By agent | `{ "agentId": "claude-1", "limit": 50 }` |
| All unclassified | `{ "all": true, "limit": 100 }` |

**Classification categories:**
| Category | Signals |
|----------|---------|
| `decision` | "decided", "chose", "agreed", "went with", "settled on" |
| `action_item` | "todo", "need to", "should", "next step", "follow up" |
| `preference` | "prefer", "always use", "never use", "convention" |
| `architecture` | "architecture", "design pattern", "structure", "api design" |
| `insight` | "learned", "realized", "turns out", "key takeaway" |
| `context` | Default — general information or background |

**Classification providers:**

| Provider | Model | Notes |
|----------|-------|-------|
| **Keyword** | — | **Default.** Offline, zero-config, instant. |
| Anthropic | claude-haiku-4-5-20251001 | Best structured output. Requires API key. |
| OpenAI | gpt-4o-mini | Cheapest external option. Requires API key. |
| Ollama | llama3.2 | Fully local, zero API cost. Requires Ollama running. |

All LLM providers gracefully fall back to keyword classification on API failure.

### Vector Search (auto-generated)

Vector search is built into the platform via `@indexed(source: "content")`. No custom endpoint needed:

```bash
# Search memories by meaning
GET /app-cortex/api/Memory?query=embedding==vector:"your search text"&limit=10

# Search with structured filters
GET /app-cortex/api/Memory?query=embedding==vector:"search"&classification==decision&agentId==claude-1&limit=10

# Search project context
GET /app-cortex/api/Synapse?query=embedding==vector:"search"&projectId==my-project&limit=10
```

### Real-Time Streaming (auto-generated)

Real-time updates are built into the platform via `@export(sse: true, mqtt: true)`:

```bash
# SSE — server-sent events
GET /app-cortex/api/Memory?stream=sse
GET /app-cortex/api/Synapse?stream=sse

# MQTT — subscribe to changes
mosquitto_sub -t "app-cortex/Memory" -h localhost -p 8883
mosquitto_sub -t "app-cortex/Synapse" -h localhost -p 8883
```

When one agent stores a memory, every subscribed agent receives it immediately.

### REST CRUD (auto-generated)

Full CRUD on all tables is auto-generated from the schema:

| Endpoint | Methods | Description |
|----------|---------|-------------|
| `/app-cortex/api/Memory` | GET, POST | List/create memories |
| `/app-cortex/api/Memory/{id}` | GET, PUT, DELETE | Read/update/delete a memory |
| `/app-cortex/api/Synapse` | GET, POST | List/create synapse entries |
| `/app-cortex/api/Synapse/{id}` | GET, PUT, DELETE | Read/update/delete a synapse entry |
| `/app-cortex/api/Settings` | GET, POST | List/create settings |
| `/app-cortex/api/Settings/{id}` | GET, PUT, DELETE | Read/update/delete settings |

### MCP Tools (auto-generated)

MCP tools for table operations are auto-generated from `@export` schemas. Any MCP-compatible agent (Claude Code, Cursor, Windsurf) can discover and use them via the standard MCP protocol at `POST /app-cortex/api/mcp`.

---

## Data Model

### Memory Table

| Field | Type | Indexed | Description |
|-------|------|---------|-------------|
| `id` | ID! | Primary key | Unique memory identifier |
| `content` | String! | Vector (HNSW) | The memory content, auto-embedded |
| `source` | String | — | Origin tag |
| `sourceId` | String | Yes | External reference for dedup |
| `agentId` | String | Yes | Originating agent |
| `channelId` | String | Yes | Conversation grouping |
| `classification` | String | Yes | Auto-assigned category |
| `entities` | String | — | JSON array of extracted entities |
| `summary` | String | — | Auto-generated one-line summary |
| `contentHash` | String! | Yes | Content fingerprint for dedup |
| `supersedes` | String | — | ID of replaced memory (provenance) |
| `createdAt` | String! | — | Creation timestamp |
| `updatedAt` | String | — | Last update timestamp |
| `metadata` | String | — | Arbitrary JSON metadata |
| `embedding` | Vector | HNSW (cosine, 384d) | Auto-generated from content |

### Synapse Table

| Field | Type | Indexed | Description |
|-------|------|---------|-------------|
| `id` | ID! | Primary key | Unique entry identifier |
| `projectId` | String! | Yes | Project scope |
| `content` | String! | Vector (HNSW) | Section content, auto-embedded |
| `source` | String! | — | File path or URL |
| `sourceFormat` | String | — | Detected format |
| `section` | String | — | Heading name within source file |
| `type` | String | Yes | "convention", "rule", "preference", "architecture", "pattern" |
| `tags` | String | — | JSON array of searchable tags |
| `entities` | String | — | JSON array of extracted entities |
| `status` | String | Yes | "active", "superseded", "archived" |
| `parentId` | String | — | Parent section for hierarchy |
| `contentHash` | String! | Yes | Content fingerprint for dedup |
| `createdAt` | String! | — | Creation timestamp |
| `updatedAt` | String | — | Last update timestamp |
| `metadata` | String | — | Arbitrary JSON metadata |
| `embedding` | Vector | HNSW (cosine, 384d) | Auto-generated from content |

### Settings Table

| Field | Type | Description |
|-------|------|-------------|
| `id` | ID! | "default" or agent/tenant-specific key |
| `classifyProvider` | String | "keyword" (default), "anthropic", "openai", "ollama" |
| `classifyModel` | String | Model identifier for LLM classification |
| `classifyApiKey` | String | API key for external LLM provider |
| `classifyEndpoint` | String | Custom endpoint URL (Ollama, proxies) |
| `dedupEnabled` | String | "true" (default) or "false" |
| `defaultSource` | String | Default source tag for new memories |
| `maxMemoriesPerAgent` | Int | Per-agent memory limit (0 = unlimited) |
| `baseUrl` | String | Self-reference URL (default http://127.0.0.1) |

---

## Configuration

### Settings (POST /app-cortex/api/Settings)

```bash
curl -X POST https://localhost:9996/app-cortex/api/Settings \
  -H "Content-Type: application/json" \
  -d '{
    "id": "default",
    "classifyProvider": "anthropic",
    "classifyModel": "claude-haiku-4-5-20251001",
    "classifyApiKey": "sk-ant-..."
  }'
```

### Embedding Models

Cortex uses the `yeti-vectors` extension for automatic embedding. The default model is `BAAI/bge-small-en-v1.5` (384 dimensions, cosine similarity). To manage available models:

```bash
# List available models
GET /yeti-vectors/models

# Download a model
POST /yeti-vectors/models
{ "model": "BAAI/bge-base-en-v1.5" }

# Set default model
PUT /yeti-vectors/models
{ "model": "BAAI/bge-base-en-v1.5", "type": "text" }
```

Supported local embedding models include:

| Model | Dimensions | Notes |
|-------|-----------|-------|
| **BAAI/bge-small-en-v1.5** | 384 | **Default.** Fast, good quality. |
| BAAI/bge-base-en-v1.5 | 768 | Higher quality, larger. |
| BAAI/bge-large-en-v1.5 | 1024 | Best quality, heaviest. |
| sentence-transformers/all-MiniLM-L6-v2 | 384 | Popular alternative. |
| Xenova/jina-embeddings-v2-small-en | 512 | Good for short text. |

All models run locally via ONNX. No API keys, no external calls, no internet required.

---

## Project Structure

```
app-cortex/
├── config.yaml              # App configuration
├── schemas/
│   └── schema.graphql       # Memory, Synapse, Settings tables
└── resources/
    ├── store.rs             # Memory storage with dedup + classification
    ├── ingest.rs            # Project context ingestion + chunking
    └── classify.rs          # LLM/keyword classification pipeline
```

---

## Authentication

Cortex uses yeti's built-in auth system. In development mode, all endpoints are accessible without authentication. In production:

- **JWT** and **Basic Auth** supported (configured in config.yaml)
- Memory and Synapse tables allow public `read` and `subscribe` access
- Write operations (store, ingest, classify) require authentication
- Settings table requires authentication for all operations

For multi-tenant deployments, use yeti-auth's role system to scope agents to their own memories via `agentId` filters.

---

## Comparison

| | app-cortex | Alternatives |
|---|---|---|
| **Deployment** | Loads with yeti, zero config | Separate services, Docker compose, cloud accounts |
| **Embeddings** | Local ONNX, automatic on write | External API calls, API keys, network dependency |
| **Offline** | Fully functional without internet | Requires cloud connectivity |
| **Classification** | Keyword default + LLM optional | LLM-only, fails without API key |
| **Real-time** | Native SSE + MQTT from schema | Custom websocket wiring or external broker |
| **Search** | Native vector search from schema | Separate vector DB or custom integration |
| **Auth** | Built-in JWT/Basic/OAuth | Custom auth implementation |
| **Binary** | Compiles to native Rust plugin | Node.js runtime + dependencies |
| **MCP** | Auto-generated from schema | Separate MCP server process |

---

Built with [Yeti](https://yetirocks.com) | The Performance Platform for Agent-Driven Development
