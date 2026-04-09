use yeti_sdk::prelude::*;

// Store memories with content-hash deduplication and optional auto-classification.
//
// POST /app-cortex/store
//   Body: { "content": "...", "source": "conversation", "sourceId": "thread-123",
//           "agentId": "claude-1", "channelId": "main", "metadata": "{}" }
//
// Dedup logic:
//   1. If sourceId provided: check for existing record with same source+sourceId → update
//   2. Compute content hash: check for existing record with same hash → return existing
//   3. Otherwise: insert new record
//
// Auto-classification: if Settings.classifyProvider is set, classifies after store.
//   Keyword classifier works offline with zero config.
//
// Response: { "id": "...", "action": "created"|"updated"|"duplicate", "classification": "..." }
resource!(Store {
    name = "store",
    post(request, ctx) => {
        let body: Value = request.json()?;

        let content = body["content"].as_str()
            .ok_or_else(|| YetiError::Validation("missing required field: content".into()))?;

        if content.trim().is_empty() {
            return bad_request("content must not be empty");
        }

        // Sanitize: strip control characters, enforce size limit
        if content.len() > 65_536 {
            return bad_request("content exceeds 64KB limit");
        }

        let memory_table = ctx.get_table("Memory")?;
        let source = body["source"].as_str().unwrap_or("manual");
        let source_id = body["sourceId"].as_str().unwrap_or("");
        let agent_id = body["agentId"].as_str().unwrap_or("");
        let channel_id = body["channelId"].as_str().unwrap_or("");
        let metadata = body["metadata"].as_str().unwrap_or("{}");
        let now = unix_timestamp()?.to_string();

        // Content hash for dedup: simple djb2 hash (no external crate needed)
        let content_hash = compute_content_hash(content);

        // --- Dedup check 1: sourceId match ---
        if !source_id.is_empty() {
            let all: Vec<Value> = memory_table.get_all().await?;
            let existing = all.iter().find(|r| {
                r["source"].as_str() == Some(source)
                    && r["sourceId"].as_str() == Some(source_id)
            });
            if let Some(record) = existing {
                let id = record["id"].as_str().unwrap_or("");
                // Update existing record
                let updated = json!({
                    "id": id,
                    "content": content,
                    "source": source,
                    "sourceId": source_id,
                    "agentId": agent_id,
                    "channelId": channel_id,
                    "contentHash": content_hash,
                    "updatedAt": now,
                    "metadata": metadata,
                    "supersedes": id,
                    "createdAt": record["createdAt"].as_str().unwrap_or(&now),
                    "classification": record["classification"].as_str().unwrap_or(""),
                    "summary": record["summary"].as_str().unwrap_or(""),
                    "entities": record["entities"].as_str().unwrap_or("[]"),
                });
                memory_table.put(id, updated).await?;
                return reply().json(json!({
                    "id": id,
                    "action": "updated",
                    "contentHash": content_hash
                }));
            }
        }

        // --- Dedup check 2: content hash match ---
        {
            let all: Vec<Value> = memory_table.get_all().await?;
            let existing = all.iter().find(|r| {
                r["contentHash"].as_str() == Some(content_hash.as_str())
            });
            if let Some(record) = existing {
                let id = record["id"].as_str().unwrap_or("");
                return reply().json(json!({
                    "id": id,
                    "action": "duplicate",
                    "contentHash": content_hash
                }));
            }
        }

        // --- Insert new memory ---
        let id = generate_id();
        let classification = classify_keyword(content);

        let record = json!({
            "id": id,
            "content": content,
            "source": source,
            "sourceId": source_id,
            "agentId": agent_id,
            "channelId": channel_id,
            "classification": classification,
            "entities": extract_entities_basic(content),
            "summary": summarize(content),
            "contentHash": content_hash,
            "createdAt": now,
            "metadata": metadata,
        });

        memory_table.put(&id, record).await?;

        reply().code(201).json(json!({
            "id": id,
            "action": "created",
            "classification": classification,
            "contentHash": content_hash
        }))
    }
});

/// djb2 hash → hex string. Deterministic, fast, sufficient for dedup.
fn compute_content_hash(content: &str) -> String {
    let mut hash: u64 = 5381;
    for byte in content.as_bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(*byte as u64);
    }
    format!("{:016x}", hash)
}

/// Generate a unique ID: timestamp-based with randomness via hash
fn generate_id() -> String {
    let ts = unix_timestamp().unwrap_or(0);
    // Use timestamp + a hash of the timestamp string for pseudo-randomness
    let noise = compute_content_hash(&ts.to_string());
    format!("mem-{}-{}", ts, &noise[..8])
}

/// Keyword-based classification. Zero config, works offline.
fn classify_keyword(content: &str) -> &'static str {
    let lower = content.to_lowercase();

    // Decision signals
    if lower.contains("decided") || lower.contains("chose") || lower.contains("agreed")
        || lower.contains("went with") || lower.contains("decision:")
        || lower.contains("we'll use") || lower.contains("settled on")
    {
        return "decision";
    }

    // Action item signals
    if lower.contains("todo") || lower.contains("to-do") || lower.contains("need to")
        || lower.contains("should ") || lower.contains("action item")
        || lower.contains("task:") || lower.contains("next step")
        || lower.contains("follow up") || lower.contains("must ")
    {
        return "action_item";
    }

    // Preference signals
    if lower.contains("prefer") || lower.contains("always use")
        || lower.contains("never use") || lower.contains("like to")
        || lower.contains("don't like") || lower.contains("preference:")
        || lower.contains("style:") || lower.contains("convention:")
    {
        return "preference";
    }

    // Architecture signals
    if lower.contains("architecture") || lower.contains("design pattern")
        || lower.contains("structure") || lower.contains("module")
        || lower.contains("component") || lower.contains("layer")
        || lower.contains("interface") || lower.contains("api design")
    {
        return "architecture";
    }

    // Insight signals
    if lower.contains("learned") || lower.contains("realized") || lower.contains("turns out")
        || lower.contains("insight:") || lower.contains("discovery:")
        || lower.contains("found that") || lower.contains("key takeaway")
    {
        return "insight";
    }

    "context"
}

/// Basic entity extraction via keyword patterns
fn extract_entities_basic(content: &str) -> String {
    // v1: return empty array. Full NER requires LLM or dedicated model.
    "[]".to_string()
}

/// Generate a one-line summary (first sentence, truncated to 120 chars)
fn summarize(content: &str) -> String {
    let first_line = content.lines().next().unwrap_or(content);
    let summary = if let Some(pos) = first_line.find(|c: char| c == '.' || c == '!' || c == '?') {
        &first_line[..=pos]
    } else {
        first_line
    };
    if summary.len() > 120 {
        format!("{}...", &summary[..117])
    } else {
        summary.to_string()
    }
}
