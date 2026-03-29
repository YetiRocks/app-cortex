use yeti_sdk::prelude::*;

// Classify or reclassify memories using an LLM or keyword fallback.
//
// POST /app-cortex/classify
//   Body: { "id": "mem-123" }                    — classify one memory
//   Body: { "agentId": "claude-1", "limit": 50 } — classify unclassified memories for an agent
//   Body: { "all": true, "limit": 100 }          — classify all unclassified memories
//
// Uses Settings.classifyProvider to determine the method:
//   "keyword"   — rule-based (default, offline, zero-config)
//   "anthropic"  — Claude API (requires classifyApiKey)
//   "openai"     — OpenAI API (requires classifyApiKey)
//   "ollama"     — Local Ollama (requires classifyEndpoint)
//
// Response: { "classified": N, "results": [{ "id": "...", "classification": "..." }] }
resource!(Classify {
    name = "classify",
    create(request, ctx) => {
        let body: Value = request.json()?;
        let memory_table = ctx.get_table("Memory")?;
        let settings_table = ctx.get_table("Settings")?;

        // Load settings
        let settings = settings_table.get("default").await?.unwrap_or(json!({}));
        let provider = settings["classifyProvider"].as_str().unwrap_or("keyword");
        let model = settings["classifyModel"].as_str().unwrap_or("");
        let api_key = settings["classifyApiKey"].as_str().unwrap_or("");
        let endpoint = settings["classifyEndpoint"].as_str().unwrap_or("");

        // Collect memories to classify
        let mut targets: Vec<Value> = Vec::new();
        let limit = body["limit"].as_u64().unwrap_or(50) as usize;

        if let Some(id) = body["id"].as_str() {
            // Single memory
            if let Some(record) = memory_table.get(id).await? {
                targets.push(record);
            } else {
                return not_found(&format!("Memory {} not found", id));
            }
        } else {
            // Batch: find unclassified or re-classify
            let all: Vec<Value> = memory_table.get_all().await?;
            let agent_filter = body["agentId"].as_str();

            for record in all {
                if targets.len() >= limit {
                    break;
                }
                let cls = record["classification"].as_str().unwrap_or("");
                if cls.is_empty() || cls == "context" {
                    if let Some(agent) = agent_filter {
                        if record["agentId"].as_str() != Some(agent) {
                            continue;
                        }
                    }
                    targets.push(record);
                }
            }
        }

        if targets.is_empty() {
            return reply().json(json!({ "classified": 0, "results": [] }));
        }

        let mut results: Vec<Value> = Vec::new();

        for target in &targets {
            let content = target["content"].as_str().unwrap_or("");
            let id = target["id"].as_str().unwrap_or("");

            let classification = match provider {
                "anthropic" => classify_anthropic(content, model, api_key)?,
                "openai" => classify_openai(content, model, api_key)?,
                "ollama" => classify_ollama(content, model, endpoint)?,
                _ => classify_keyword(content).to_string(),
            };

            // Update the record
            let mut updated = target.clone();
            updated["classification"] = json!(classification);
            updated["updatedAt"] = json!(unix_timestamp()?.to_string());
            memory_table.put(id, updated).await?;

            results.push(json!({
                "id": id,
                "classification": classification
            }));
        }

        reply().json(json!({
            "classified": results.len(),
            "results": results
        }))
    }
});

const CLASSIFY_PROMPT: &str = r#"Classify this text into exactly one category. Respond with ONLY the category name, nothing else.

Categories:
- decision: A choice or decision that was made
- action_item: A task, to-do, or next step
- preference: A stated preference, convention, or style choice
- architecture: A design decision, system structure, or technical pattern
- insight: A learning, realization, or discovery
- context: General information or background

Text: "#;

fn classify_anthropic(content: &str, model: &str, api_key: &str) -> Result<String> {
    if api_key.is_empty() {
        return Ok(classify_keyword(content).to_string());
    }
    let model = if model.is_empty() { "claude-haiku-4-5-20251001" } else { model };
    let body = json!({
        "model": model,
        "max_tokens": 32,
        "messages": [{"role": "user", "content": format!("{}{}", CLASSIFY_PROMPT, truncate(content, 2000))}]
    });

    let resp = fetch("https://api.anthropic.com/v1/messages", Some(json!({
        "method": "POST",
        "headers": {
            "x-api-key": api_key,
            "anthropic-version": "2023-06-01",
            "content-type": "application/json"
        },
        "body": body.to_string()
    })))?;

    if !resp.ok() {
        yeti_log!(warn, "Anthropic API error {}: {}", resp.status, resp.body);
        return Ok(classify_keyword(content).to_string());
    }

    let parsed: Value = serde_json::from_str(&resp.body)
        .unwrap_or(json!({}));
    let text = parsed["content"][0]["text"].as_str().unwrap_or("");
    Ok(normalize_classification(text))
}

fn classify_openai(content: &str, model: &str, api_key: &str) -> Result<String> {
    if api_key.is_empty() {
        return Ok(classify_keyword(content).to_string());
    }
    let model = if model.is_empty() { "gpt-4o-mini" } else { model };
    let body = json!({
        "model": model,
        "max_tokens": 32,
        "messages": [{"role": "user", "content": format!("{}{}", CLASSIFY_PROMPT, truncate(content, 2000))}]
    });

    let resp = fetch("https://api.openai.com/v1/chat/completions", Some(json!({
        "method": "POST",
        "headers": {
            "Authorization": format!("Bearer {}", api_key),
            "Content-Type": "application/json"
        },
        "body": body.to_string()
    })))?;

    if !resp.ok() {
        yeti_log!(warn, "OpenAI API error {}: {}", resp.status, resp.body);
        return Ok(classify_keyword(content).to_string());
    }

    let parsed: Value = serde_json::from_str(&resp.body)
        .unwrap_or(json!({}));
    let text = parsed["choices"][0]["message"]["content"].as_str().unwrap_or("");
    Ok(normalize_classification(text))
}

fn classify_ollama(content: &str, model: &str, endpoint: &str) -> Result<String> {
    let endpoint = if endpoint.is_empty() { "http://127.0.0.1:11434" } else { endpoint };
    let model = if model.is_empty() { "llama3.2" } else { model };
    let url = format!("{}/api/generate", endpoint);
    let body = json!({
        "model": model,
        "prompt": format!("{}{}", CLASSIFY_PROMPT, truncate(content, 2000)),
        "stream": false,
        "options": { "num_predict": 32 }
    });

    let resp = fetch(&url, Some(json!({
        "method": "POST",
        "headers": { "Content-Type": "application/json" },
        "body": body.to_string()
    })))?;

    if !resp.ok() {
        yeti_log!(warn, "Ollama API error {}: {}", resp.status, resp.body);
        return Ok(classify_keyword(content).to_string());
    }

    let parsed: Value = serde_json::from_str(&resp.body)
        .unwrap_or(json!({}));
    let text = parsed["response"].as_str().unwrap_or("");
    Ok(normalize_classification(text))
}

const VALID_CLASSES: &[&str] = &[
    "decision", "action_item", "preference", "architecture", "insight", "context"
];

fn normalize_classification(raw: &str) -> String {
    let cleaned = raw.trim().to_lowercase().replace(['-', ' '], "_");
    if VALID_CLASSES.contains(&cleaned.as_str()) {
        cleaned
    } else {
        // Fuzzy match
        for cls in VALID_CLASSES {
            if cleaned.contains(cls) {
                return cls.to_string();
            }
        }
        "context".to_string()
    }
}

fn classify_keyword(content: &str) -> &'static str {
    let lower = content.to_lowercase();
    if lower.contains("decided") || lower.contains("chose") || lower.contains("agreed")
        || lower.contains("went with") || lower.contains("decision:")
        || lower.contains("we'll use") || lower.contains("settled on")
    {
        return "decision";
    }
    if lower.contains("todo") || lower.contains("to-do") || lower.contains("need to")
        || lower.contains("should ") || lower.contains("action item")
        || lower.contains("task:") || lower.contains("next step")
        || lower.contains("follow up") || lower.contains("must ")
    {
        return "action_item";
    }
    if lower.contains("prefer") || lower.contains("always use")
        || lower.contains("never use") || lower.contains("like to")
        || lower.contains("don't like") || lower.contains("preference:")
    {
        return "preference";
    }
    if lower.contains("architecture") || lower.contains("design pattern")
        || lower.contains("structure") || lower.contains("api design")
    {
        return "architecture";
    }
    if lower.contains("learned") || lower.contains("realized") || lower.contains("turns out")
        || lower.contains("insight:") || lower.contains("found that")
    {
        return "insight";
    }
    "context"
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}
