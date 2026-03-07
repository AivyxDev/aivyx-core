//! Relay operations — cross-instance chat, task creation, and federated search.

use aivyx_core::AivyxError;

use crate::client::FederationClient;
use crate::types::{
    FederatedSearchRequest, FederatedSearchResult, RelayChatRequest, RelayChatResponse,
    RelayTaskRequest, RelayTaskResponse,
};

impl FederationClient {
    /// Relay a chat message to a peer's agent.
    pub async fn relay_chat(
        &self,
        req: &RelayChatRequest,
    ) -> Result<RelayChatResponse, AivyxError> {
        let peer_config = self.get_peer_config(&req.peer_id).await?;

        let url = format!("{}/chat", peer_config.url.trim_end_matches('/'));
        let body = serde_json::json!({
            "agent": req.agent,
            "message": req.message,
            "session_id": req.session_id,
        });
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| AivyxError::Other(format!("serialize chat relay: {e}")))?;

        let header = self.auth().sign_request(&body_bytes);

        let resp = self
            .http()
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Federation-Instance", &header.instance_id)
            .header("X-Federation-Timestamp", header.timestamp.to_string())
            .header("X-Federation-Signature", &header.signature)
            // Also send the bearer token if the peer requires standard auth
            .header("Authorization", format!("Bearer {}", "federation"))
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| AivyxError::Other(format!("relay chat to {}: {e}", req.peer_id)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AivyxError::Other(format!(
                "peer {} chat returned {}: {}",
                req.peer_id, status, text
            )));
        }

        let chat_resp: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AivyxError::Other(format!("parse chat response: {e}")))?;

        Ok(RelayChatResponse {
            peer_id: req.peer_id.clone(),
            agent: req.agent.clone(),
            response: chat_resp["response"].as_str().unwrap_or("").to_string(),
            session_id: chat_resp["session_id"].as_str().unwrap_or("").to_string(),
        })
    }

    /// Create a task on a peer instance.
    pub async fn relay_task(
        &self,
        req: &RelayTaskRequest,
    ) -> Result<RelayTaskResponse, AivyxError> {
        let peer_config = self.get_peer_config(&req.peer_id).await?;

        let url = format!("{}/tasks", peer_config.url.trim_end_matches('/'));
        let body = serde_json::json!({
            "agent": req.agent,
            "goal": req.goal,
        });
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| AivyxError::Other(format!("serialize task relay: {e}")))?;

        let header = self.auth().sign_request(&body_bytes);

        let resp = self
            .http()
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Federation-Instance", &header.instance_id)
            .header("X-Federation-Timestamp", header.timestamp.to_string())
            .header("X-Federation-Signature", &header.signature)
            .header("Authorization", format!("Bearer {}", "federation"))
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| AivyxError::Other(format!("relay task to {}: {e}", req.peer_id)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(AivyxError::Other(format!(
                "peer {} task returned {}: {}",
                req.peer_id, status, text
            )));
        }

        let task_resp: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| AivyxError::Other(format!("parse task response: {e}")))?;

        Ok(RelayTaskResponse {
            peer_id: req.peer_id.clone(),
            task_id: task_resp["id"].as_str().unwrap_or("").to_string(),
            status: task_resp["status"]
                .as_str()
                .unwrap_or("created")
                .to_string(),
        })
    }

    /// Relay a chat message with automatic failover.
    /// Tries the preferred peer first (from req.peer_id), then falls back to
    /// other healthy peers with the given capability.
    pub async fn relay_chat_with_failover(
        &self,
        req: &RelayChatRequest,
        capability: &str,
    ) -> Result<RelayChatResponse, AivyxError> {
        let candidates = self
            .build_failover_candidates(&req.peer_id, capability)
            .await;

        let mut last_error = AivyxError::Other("no failover candidates available".into());

        for peer_id in &candidates {
            let mut patched_req = req.clone();
            patched_req.peer_id = peer_id.clone();

            match self.relay_chat(&patched_req).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    if is_retryable_error(&e) {
                        self.mark_unhealthy(peer_id).await;
                        tracing::warn!(peer = %peer_id, error = %e, "failover: retryable error, trying next peer");
                        last_error = e;
                    } else {
                        // 4xx or non-retryable error: return immediately.
                        return Err(e);
                    }
                }
            }
        }

        Err(last_error)
    }

    /// Relay a task with automatic failover.
    pub async fn relay_task_with_failover(
        &self,
        req: &RelayTaskRequest,
        capability: &str,
    ) -> Result<RelayTaskResponse, AivyxError> {
        let candidates = self
            .build_failover_candidates(&req.peer_id, capability)
            .await;

        let mut last_error = AivyxError::Other("no failover candidates available".into());

        for peer_id in &candidates {
            let mut patched_req = req.clone();
            patched_req.peer_id = peer_id.clone();

            match self.relay_task(&patched_req).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    if is_retryable_error(&e) {
                        self.mark_unhealthy(peer_id).await;
                        tracing::warn!(peer = %peer_id, error = %e, "failover: retryable error, trying next peer");
                        last_error = e;
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(last_error)
    }

    /// Build the ordered list of failover candidates.
    /// Preferred peer first (if healthy), then other healthy peers for the capability,
    /// truncated to max_attempts.
    async fn build_failover_candidates(&self, preferred: &str, capability: &str) -> Vec<String> {
        build_candidate_list(
            preferred,
            &self.healthy_peers_for(capability).await,
            self.failover_config().max_attempts,
        )
    }

    /// Search memory across federated peers.
    pub async fn federated_search(
        &self,
        req: &FederatedSearchRequest,
    ) -> Result<Vec<FederatedSearchResult>, AivyxError> {
        let peer_ids: Vec<String> = if req.peers.is_empty() {
            // Search all healthy peers
            let peers = self.list_peers().await;
            peers
                .into_iter()
                .filter(|p| p.healthy && p.capabilities.contains(&"memory".to_string()))
                .map(|p| p.id)
                .collect()
        } else {
            req.peers.clone()
        };

        let mut all_results = Vec::new();

        for peer_id in &peer_ids {
            match self.search_peer(peer_id, &req.query, req.limit).await {
                Ok(mut results) => {
                    all_results.append(&mut results);
                }
                Err(e) => {
                    tracing::warn!(peer = %peer_id, error = %e, "federated search failed for peer");
                }
            }
        }

        // Sort by score descending
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Apply global limit
        if let Some(limit) = req.limit {
            all_results.truncate(limit);
        }

        Ok(all_results)
    }

    /// Search a single peer's memory.
    async fn search_peer(
        &self,
        peer_id: &str,
        query: &str,
        limit: Option<usize>,
    ) -> Result<Vec<FederatedSearchResult>, AivyxError> {
        let peer_config = self.get_peer_config(peer_id).await?;

        let url = format!("{}/memory/search", peer_config.url.trim_end_matches('/'));
        let body = serde_json::json!({
            "query": query,
            "limit": limit.unwrap_or(10),
        });
        let body_bytes = serde_json::to_vec(&body)
            .map_err(|e| AivyxError::Other(format!("serialize search: {e}")))?;

        let header = self.auth().sign_request(&body_bytes);

        let resp = self
            .http()
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Federation-Instance", &header.instance_id)
            .header("X-Federation-Timestamp", header.timestamp.to_string())
            .header("X-Federation-Signature", &header.signature)
            .header("Authorization", format!("Bearer {}", "federation"))
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| AivyxError::Other(format!("search peer {peer_id}: {e}")))?;

        if !resp.status().is_success() {
            return Err(AivyxError::Other(format!(
                "peer {peer_id} search returned {}",
                resp.status()
            )));
        }

        let search_results: Vec<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| AivyxError::Other(format!("parse search results: {e}")))?;

        Ok(search_results
            .into_iter()
            .map(|r| FederatedSearchResult {
                peer_id: peer_id.to_string(),
                content: r["content"].as_str().unwrap_or("").to_string(),
                score: r["score"].as_f64().unwrap_or(0.0),
                kind: r["kind"].as_str().unwrap_or("unknown").to_string(),
            })
            .collect())
    }
}

/// Check if an error is retryable (connection error or 5xx status).
fn is_retryable_error(err: &AivyxError) -> bool {
    let msg = err.to_string();
    // Connection-related errors from reqwest
    if msg.contains("connection")
        || msg.contains("Connection")
        || msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("dns")
        || msg.contains("DNS")
        || msg.contains("unreachable")
    {
        return true;
    }
    // 5xx status codes in error messages
    for code in [500, 501, 502, 503, 504] {
        if msg.contains(&format!("{code}")) {
            // Make sure it looks like a status code context, not a random number.
            if msg.contains(&format!("returned {code}"))
                || msg.contains(&format!("{code} "))
                || msg.contains(&format!("{code}:"))
            {
                return true;
            }
        }
    }
    false
}

/// Build the ordered candidate list for failover.
/// Preferred peer first (if present in healthy list), then remaining healthy peers,
/// truncated to max_attempts.
fn build_candidate_list(preferred: &str, healthy: &[String], max_attempts: usize) -> Vec<String> {
    let mut candidates = Vec::new();

    // Preferred peer first, if it's in the healthy list.
    if healthy.contains(&preferred.to_string()) {
        candidates.push(preferred.to_string());
    }

    // Then the rest, excluding preferred.
    for peer in healthy {
        if peer != preferred {
            candidates.push(peer.clone());
        }
    }

    candidates.truncate(max_attempts);
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failover_builds_candidate_list() {
        // Preferred peer is healthy and should be first.
        let healthy = vec!["b".to_string(), "a".to_string(), "c".to_string()];
        let candidates = build_candidate_list("a", &healthy, 3);
        assert_eq!(candidates[0], "a");
        assert_eq!(candidates.len(), 3);

        // Preferred peer not in healthy list.
        let candidates = build_candidate_list("x", &healthy, 3);
        assert_eq!(candidates, vec!["b", "a", "c"]);

        // max_attempts truncates.
        let candidates = build_candidate_list("a", &healthy, 2);
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0], "a");

        // Empty healthy list.
        let candidates = build_candidate_list("a", &[], 3);
        assert!(candidates.is_empty());
    }

    #[test]
    fn retryable_error_detection() {
        assert!(is_retryable_error(&AivyxError::Other(
            "relay chat to peer-a: connection refused".into()
        )));
        assert!(is_retryable_error(&AivyxError::Other(
            "peer peer-a chat returned 502: bad gateway".into()
        )));
        assert!(is_retryable_error(&AivyxError::Other(
            "peer peer-a chat returned 503: service unavailable".into()
        )));
        assert!(is_retryable_error(&AivyxError::Other(
            "request timed out".into()
        )));
        // 4xx should not be retryable.
        assert!(!is_retryable_error(&AivyxError::Other(
            "peer peer-a chat returned 400: bad request".into()
        )));
        assert!(!is_retryable_error(&AivyxError::Other(
            "peer peer-a chat returned 404: not found".into()
        )));
    }
}
