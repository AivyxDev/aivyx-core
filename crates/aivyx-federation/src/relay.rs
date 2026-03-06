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
