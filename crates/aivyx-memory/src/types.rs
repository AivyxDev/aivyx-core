//! Data types for the memory subsystem.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use aivyx_core::{AgentId, MemoryId, TripleId};

/// The kind of memory being stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryKind {
    /// A factual assertion.
    Fact,
    /// A user or agent preference.
    Preference,
    /// A summary of a previous session.
    SessionSummary,
    /// A procedure or how-to.
    Procedure,
    /// A decision with rationale.
    Decision,
    /// An outcome summary.
    Outcome,
    /// Any other kind of memory.
    Custom(String),
}

/// A single memory entry stored by an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier.
    pub id: MemoryId,
    /// The textual content of this memory.
    pub content: String,
    /// What kind of memory this is.
    pub kind: MemoryKind,
    /// If set, this memory is private to the specified agent.
    /// `None` means globally visible.
    pub agent_scope: Option<AgentId>,
    /// Free-form tags for categorization.
    pub tags: Vec<String>,
    /// Binary attachments (images, audio) associated with this memory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MemoryAttachment>,
    /// When this memory was first created.
    pub created_at: DateTime<Utc>,
    /// When this memory was last updated.
    pub updated_at: DateTime<Utc>,
    /// How many times this memory has been retrieved.
    pub access_count: u64,
    /// When this memory was last accessed via recall.
    pub last_accessed_at: Option<DateTime<Utc>>,
}

/// A binary attachment associated with a memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryAttachment {
    /// Unique identifier for this attachment.
    pub id: String,
    /// MIME type (e.g., "image/png", "audio/wav").
    pub media_type: String,
    /// LLM-generated description of the attachment content.
    /// Used for embedding and search (description-based image embedding).
    pub description: Option<String>,
}

impl MemoryEntry {
    /// Create a new memory entry with the current timestamp.
    pub fn new(
        content: String,
        kind: MemoryKind,
        agent_scope: Option<AgentId>,
        tags: Vec<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: MemoryId::new(),
            content,
            kind,
            agent_scope,
            tags,
            attachments: Vec::new(),
            created_at: now,
            updated_at: now,
            access_count: 0,
            last_accessed_at: None,
        }
    }

    /// Create a new memory entry with attachments.
    pub fn with_attachments(
        content: String,
        kind: MemoryKind,
        agent_scope: Option<AgentId>,
        tags: Vec<String>,
        attachments: Vec<MemoryAttachment>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: MemoryId::new(),
            content,
            kind,
            agent_scope,
            tags,
            attachments,
            created_at: now,
            updated_at: now,
            access_count: 0,
            last_accessed_at: None,
        }
    }

    /// Record an access, bumping the counter and timestamp.
    pub fn record_access(&mut self) {
        self.access_count += 1;
        self.last_accessed_at = Some(Utc::now());
    }
}

/// A subject-predicate-object knowledge triple.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeTriple {
    /// Unique identifier.
    pub id: TripleId,
    /// The subject entity (e.g., "Rust").
    pub subject: String,
    /// The relationship (e.g., "is_a").
    pub predicate: String,
    /// The object entity (e.g., "programming language").
    pub object: String,
    /// If set, this triple is private to the specified agent.
    pub agent_scope: Option<AgentId>,
    /// Confidence score (0.0 to 1.0).
    pub confidence: f32,
    /// Where this knowledge came from (e.g., "user", "derived").
    pub source: String,
    /// IDs of attachments associated with this triple (e.g., images
    /// that were the source of visual knowledge).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachment_ids: Vec<String>,
    /// When this triple was created.
    pub created_at: DateTime<Utc>,
}

impl KnowledgeTriple {
    /// Create a new knowledge triple with the current timestamp.
    pub fn new(
        subject: String,
        predicate: String,
        object: String,
        agent_scope: Option<AgentId>,
        confidence: f32,
        source: String,
    ) -> Self {
        Self {
            id: TripleId::new(),
            subject,
            predicate,
            object,
            agent_scope,
            confidence,
            source,
            attachment_ids: Vec::new(),
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_entry_serde_roundtrip() {
        let entry = MemoryEntry::new(
            "Rust is fast".into(),
            MemoryKind::Fact,
            None,
            vec!["programming".into()],
        );
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: MemoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, entry.id);
        assert_eq!(parsed.content, "Rust is fast");
        assert_eq!(parsed.kind, MemoryKind::Fact);
        assert!(parsed.agent_scope.is_none());
        assert_eq!(parsed.tags, vec!["programming"]);
        assert_eq!(parsed.access_count, 0);
    }

    #[test]
    fn knowledge_triple_serde_roundtrip() {
        let triple = KnowledgeTriple::new(
            "Rust".into(),
            "is_a".into(),
            "programming language".into(),
            None,
            0.95,
            "user".into(),
        );
        let json = serde_json::to_string(&triple).unwrap();
        let parsed: KnowledgeTriple = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.subject, "Rust");
        assert_eq!(parsed.predicate, "is_a");
        assert_eq!(parsed.object, "programming language");
        assert!((parsed.confidence - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn memory_kind_serde_roundtrip() {
        let kinds = vec![
            MemoryKind::Fact,
            MemoryKind::Preference,
            MemoryKind::SessionSummary,
            MemoryKind::Procedure,
            MemoryKind::Decision,
            MemoryKind::Outcome,
            MemoryKind::Custom("workflow".into()),
        ];
        for kind in kinds {
            let json = serde_json::to_string(&kind).unwrap();
            let parsed: MemoryKind = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, kind);
        }
    }

    #[test]
    fn decision_and_outcome_kinds_serde() {
        let decision = MemoryKind::Decision;
        let json = serde_json::to_string(&decision).unwrap();
        assert_eq!(json, r#""Decision""#);
        let parsed: MemoryKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, MemoryKind::Decision);

        let outcome = MemoryKind::Outcome;
        let json = serde_json::to_string(&outcome).unwrap();
        assert_eq!(json, r#""Outcome""#);
        let parsed: MemoryKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, MemoryKind::Outcome);
    }

    #[test]
    fn record_access_increments() {
        let mut entry = MemoryEntry::new("test".into(), MemoryKind::Fact, None, vec![]);
        assert_eq!(entry.access_count, 0);
        assert!(entry.last_accessed_at.is_none());

        entry.record_access();
        assert_eq!(entry.access_count, 1);
        assert!(entry.last_accessed_at.is_some());

        entry.record_access();
        assert_eq!(entry.access_count, 2);
    }

    #[test]
    fn memory_entry_with_attachments() {
        let entry = MemoryEntry::with_attachments(
            "A photo of a sunset".into(),
            MemoryKind::Fact,
            None,
            vec!["photo".into()],
            vec![MemoryAttachment {
                id: "attach-1".into(),
                media_type: "image/jpeg".into(),
                description: Some("A vibrant sunset over the ocean".into()),
            }],
        );
        assert_eq!(entry.attachments.len(), 1);
        assert_eq!(entry.attachments[0].id, "attach-1");
        assert_eq!(entry.attachments[0].media_type, "image/jpeg");
        assert_eq!(
            entry.attachments[0].description.as_deref(),
            Some("A vibrant sunset over the ocean")
        );
    }

    #[test]
    fn memory_entry_attachments_backward_compat() {
        // Existing entries without attachments should deserialize fine
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "content": "old memory",
            "kind": "Fact",
            "agent_scope": null,
            "tags": [],
            "created_at": "2024-01-01T00:00:00Z",
            "updated_at": "2024-01-01T00:00:00Z",
            "access_count": 0,
            "last_accessed_at": null
        }"#;
        let entry: MemoryEntry = serde_json::from_str(json).unwrap();
        assert!(entry.attachments.is_empty());
    }

    #[test]
    fn memory_entry_attachments_serde_roundtrip() {
        let entry = MemoryEntry::with_attachments(
            "test".into(),
            MemoryKind::Fact,
            None,
            vec![],
            vec![MemoryAttachment {
                id: "img-1".into(),
                media_type: "image/png".into(),
                description: None,
            }],
        );
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("attachments"));
        let parsed: MemoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.attachments.len(), 1);
    }

    #[test]
    fn memory_entry_no_attachments_omits_field() {
        let entry = MemoryEntry::new("test".into(), MemoryKind::Fact, None, vec![]);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("attachments")); // skip_serializing_if = "Vec::is_empty"
    }

    #[test]
    fn knowledge_triple_with_attachment_ids() {
        let mut triple = KnowledgeTriple::new(
            "cat".into(),
            "looks_like".into(),
            "fluffy animal".into(),
            None,
            0.9,
            "vision".into(),
        );
        triple.attachment_ids.push("img-1".into());
        let json = serde_json::to_string(&triple).unwrap();
        assert!(json.contains("attachment_ids"));
        let parsed: KnowledgeTriple = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.attachment_ids, vec!["img-1"]);
    }

    #[test]
    fn knowledge_triple_backward_compat() {
        // Existing triples without attachment_ids
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "subject": "Rust",
            "predicate": "is_a",
            "object": "language",
            "agent_scope": null,
            "confidence": 0.95,
            "source": "user",
            "created_at": "2024-01-01T00:00:00Z"
        }"#;
        let triple: KnowledgeTriple = serde_json::from_str(json).unwrap();
        assert!(triple.attachment_ids.is_empty());
    }
}
