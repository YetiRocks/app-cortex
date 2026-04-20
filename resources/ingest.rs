use yeti_sdk::prelude::*;

// Ingest project context files into the Synapse table.
//
// POST /app-cortex/ingest
//   Body: { "projectId": "my-project", "source": ".claude/CLAUDE.md",
//           "sourceFormat": "claude-md", "content": "# Project\n## Rules\n...",
//           "tags": "[\"rust\", \"auth\"]" }
//
// Chunks Markdown by ## headings. Each chunk becomes a Synapse record with
// auto-generated embeddings (via @indexed). Deduplicates by content hash
// per project+source, replacing stale sections on re-ingest.
//
// Response: { "projectId": "...", "source": "...", "inserted": N, "updated": N, "unchanged": N }
resource!(Ingest {
    name = "ingest",
    post(ctx) => {
        let body: Value = ctx.require_json_body()?.clone();

        let project_id = body["projectId"].as_str()
            .ok_or_else(|| YetiError::Validation("missing required field: projectId".into()))?;
        let source = body["source"].as_str()
            .ok_or_else(|| YetiError::Validation("missing required field: source".into()))?;
        let content = body["content"].as_str()
            .ok_or_else(|| YetiError::Validation("missing required field: content".into()))?;

        if content.len() > 1_048_576 {
            return bad_request("content exceeds 1MB limit");
        }

        let source_format = body["sourceFormat"].as_str().unwrap_or(
            detect_format(source)
        );
        let tags = body["tags"].as_str().unwrap_or("[]");
        let now = unix_timestamp()?.to_string();

        let synapse_table = ctx.table("Synapse")?;

        // Chunk content by ## headings
        let chunks = chunk_markdown(content);

        // Load existing records for this project+source to detect updates
        let all: Vec<Value> = synapse_table.get_all().await?;
        let existing: Vec<&Value> = all.iter().filter(|r| {
            r["projectId"].as_str() == Some(project_id)
                && r["source"].as_str() == Some(source)
        }).collect();

        let mut inserted = 0u32;
        let mut updated = 0u32;
        let mut unchanged = 0u32;

        for chunk in &chunks {
            let hash = compute_hash(&chunk.content);

            // Check if this section already exists
            let existing_record = existing.iter().find(|r| {
                r["section"].as_str() == Some(&chunk.heading)
            });

            if let Some(record) = existing_record {
                // Same content hash → unchanged
                if record["contentHash"].as_str() == Some(hash.as_str()) {
                    unchanged += 1;
                    continue;
                }
                // Different content → update
                let id = record["id"].as_str().unwrap_or("");
                let updated_record = json!({
                    "id": id,
                    "projectId": project_id,
                    "content": chunk.content,
                    "source": source,
                    "sourceFormat": source_format,
                    "section": chunk.heading,
                    "type": classify_section(&chunk.heading, &chunk.content),
                    "tags": tags,
                    "entities": "[]",
                    "status": "active",
                    "parentId": chunk.parent.as_deref().unwrap_or(""),
                    "contentHash": hash,
                    "createdAt": record["createdAt"].as_str().unwrap_or(&now),
                    "updatedAt": now,
                    "metadata": "{}",
                });
                synapse_table.put(id, updated_record).await?;
                updated += 1;
            } else {
                // New section → insert
                let id = format!("syn-{}-{}", unix_timestamp()?, &hash[..8]);
                let new_record = json!({
                    "id": id,
                    "projectId": project_id,
                    "content": chunk.content,
                    "source": source,
                    "sourceFormat": source_format,
                    "section": chunk.heading,
                    "type": classify_section(&chunk.heading, &chunk.content),
                    "tags": tags,
                    "entities": "[]",
                    "status": "active",
                    "parentId": chunk.parent.as_deref().unwrap_or(""),
                    "contentHash": hash,
                    "createdAt": now,
                    "metadata": "{}",
                });
                synapse_table.put(&id, new_record).await?;
                inserted += 1;
            }
        }

        // Mark sections that no longer exist in the source as superseded
        let current_headings: Vec<&str> = chunks.iter().map(|c| c.heading.as_str()).collect();
        for record in &existing {
            if let Some(section) = record["section"].as_str() {
                if !current_headings.contains(&section) {
                    if let Some(id) = record["id"].as_str() {
                        let mut archived = record.clone().clone();
                        archived["status"] = json!("superseded");
                        archived["updatedAt"] = json!(&now);
                        synapse_table.put(id, archived).await?;
                    }
                }
            }
        }

        ok(json!({
            "projectId": project_id,
            "source": source,
            "sourceFormat": source_format,
            "chunks": chunks.len(),
            "inserted": inserted,
            "updated": updated,
            "unchanged": unchanged
        }))
    }
});

struct Chunk {
    heading: String,
    content: String,
    parent: Option<String>,
}

/// Split Markdown by ## headings. Content before the first heading becomes "preamble".
fn chunk_markdown(content: &str) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut current_heading = String::from("preamble");
    let mut current_content = String::new();
    let mut current_parent: Option<String> = None;
    let mut last_h1: Option<String> = None;

    for line in content.lines() {
        if line.starts_with("# ") && !line.starts_with("## ") {
            // H1: flush current, start new section, track as parent
            if !current_content.trim().is_empty() {
                chunks.push(Chunk {
                    heading: current_heading.clone(),
                    content: current_content.trim().to_string(),
                    parent: current_parent.clone(),
                });
            }
            current_heading = line.trim_start_matches("# ").trim().to_string();
            last_h1 = Some(current_heading.clone());
            current_parent = None;
            current_content = String::new();
        } else if line.starts_with("## ") {
            // H2: flush current, start new section
            if !current_content.trim().is_empty() {
                chunks.push(Chunk {
                    heading: current_heading.clone(),
                    content: current_content.trim().to_string(),
                    parent: current_parent.clone(),
                });
            }
            current_heading = line.trim_start_matches("## ").trim().to_string();
            current_parent = last_h1.clone();
            current_content = String::new();
        } else {
            current_content.push_str(line);
            current_content.push('\n');
        }
    }

    // Flush final section
    if !current_content.trim().is_empty() {
        chunks.push(Chunk {
            heading: current_heading,
            content: current_content.trim().to_string(),
            parent: current_parent,
        });
    }

    chunks
}

fn detect_format(source: &str) -> &'static str {
    let lower = source.to_lowercase();
    if lower.contains("claude") && lower.ends_with(".md") {
        "claude-md"
    } else if lower.contains(".cursor") || lower.contains("cursor") {
        "cursor-rules"
    } else if lower.contains(".windsurf") || lower.contains("windsurf") {
        "windsurf"
    } else if lower.ends_with(".md") {
        "markdown"
    } else {
        "custom"
    }
}

fn classify_section(heading: &str, content: &str) -> &'static str {
    let h = heading.to_lowercase();
    let c = content.to_lowercase();

    if h.contains("rule") || h.contains("constraint") || c.contains("must ") || c.contains("never ") {
        "rule"
    } else if h.contains("convention") || h.contains("style") || h.contains("format") {
        "convention"
    } else if h.contains("prefer") || c.contains("prefer") {
        "preference"
    } else if h.contains("architect") || h.contains("design") || h.contains("structure") {
        "architecture"
    } else if h.contains("pattern") || h.contains("example") {
        "pattern"
    } else {
        "convention"
    }
}

fn compute_hash(content: &str) -> String {
    let mut hash: u64 = 5381;
    for byte in content.as_bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(*byte as u64);
    }
    format!("{:016x}", hash)
}
