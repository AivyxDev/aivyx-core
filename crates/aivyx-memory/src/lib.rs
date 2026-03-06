//! Memory and knowledge subsystem for the aivyx framework.
//!
//! Provides encrypted vector storage, cosine similarity search, and knowledge
//! triple persistence. All data is encrypted at rest using a domain-separated
//! key derived from the master key.

pub mod manager;
pub mod notification;
pub mod profile;
pub mod profile_extractor;
pub mod search;
pub mod store;
pub mod tools;
pub mod types;

pub use manager::{MemoryManager, MemoryStats};
pub use notification::{Notification, NotificationStore, Rating};
pub use profile::{PROFILE_VERSION, ProjectEntry, RecurringTask, UserProfile};
pub use search::{SearchResult, VectorIndex, content_hash, cosine_similarity};
pub use store::MemoryStore;
pub use tools::register_memory_tools;
pub use types::{KnowledgeTriple, MemoryEntry, MemoryKind};
