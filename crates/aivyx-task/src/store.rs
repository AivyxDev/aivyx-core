//! Encrypted task persistence.
//!
//! [`TaskStore`] wraps [`EncryptedStore`] with a `"task:{TaskId}"` key namespace.
//! Follows the same pattern as `NotificationStore` in `aivyx-memory`.

use std::path::Path;

use aivyx_core::{Result, TaskId};
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Utc};

use crate::status::TaskStatus;
use crate::task::Task;

/// Query filter for listing tasks.
#[derive(Debug, Default)]
pub struct TaskFilter {
    /// Only return tasks with this status.
    pub status: Option<TaskStatus>,
    /// Only return tasks owned by this agent.
    pub agent_name: Option<String>,
    /// Only return tasks created after this timestamp.
    pub created_after: Option<DateTime<Utc>>,
    /// Only return tasks created before this timestamp.
    pub created_before: Option<DateTime<Utc>>,
    /// Maximum number of tasks to return.
    pub limit: Option<usize>,
}

/// Encrypted store for task records.
pub struct TaskStore {
    inner: EncryptedStore,
}

impl TaskStore {
    /// Open or create a task store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let inner = EncryptedStore::open(path)?;
        Ok(Self { inner })
    }

    /// Create a task store wrapping an existing `EncryptedStore`.
    pub fn new(inner: EncryptedStore) -> Self {
        Self { inner }
    }

    /// Persist a task.
    pub fn save(&self, task: &Task, key: &MasterKey) -> Result<()> {
        let store_key = format!("task:{}", task.id);
        let json = serde_json::to_vec(task)?;
        self.inner.put(&store_key, &json, key)
    }

    /// Retrieve a task by ID. Returns `None` if not found.
    pub fn get(&self, id: &TaskId, key: &MasterKey) -> Result<Option<Task>> {
        let store_key = format!("task:{id}");
        match self.inner.get(&store_key, key)? {
            Some(bytes) => {
                let task: Task = serde_json::from_slice(&bytes)?;
                Ok(Some(task))
            }
            None => Ok(None),
        }
    }

    /// Atomically read-modify-write a task.
    ///
    /// Loads the task, applies `f`, and saves the result in a single logical
    /// operation. Returns the updated task, or an error if the task is not found.
    pub fn update(
        &self,
        id: &TaskId,
        key: &MasterKey,
        f: impl FnOnce(&mut Task),
    ) -> Result<Task> {
        let mut task = self
            .get(id, key)?
            .ok_or_else(|| aivyx_core::AivyxError::Other(format!("task not found: {id}")))?;
        f(&mut task);
        self.save(&task, key)?;
        Ok(task)
    }

    /// Delete a task by ID.
    pub fn delete(&self, id: &TaskId) -> Result<()> {
        let store_key = format!("task:{id}");
        self.inner.delete(&store_key)
    }

    /// List all tasks sorted by creation time (newest first).
    pub fn list(&self, key: &MasterKey) -> Result<Vec<Task>> {
        let keys = self.inner.list_keys()?;
        let mut tasks = Vec::new();
        for k in keys {
            if k.starts_with("task:")
                && let Some(bytes) = self.inner.get(&k, key)?
            {
                let task: Task = serde_json::from_slice(&bytes)?;
                tasks.push(task);
            }
        }
        tasks.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(tasks)
    }

    /// List tasks matching a filter, sorted by creation time (newest first).
    pub fn query(&self, filter: &TaskFilter, key: &MasterKey) -> Result<Vec<Task>> {
        let all = self.list(key)?;
        let filtered = all
            .into_iter()
            .filter(|t| filter.status.is_none_or(|s| t.status == s))
            .filter(|t| {
                filter
                    .agent_name
                    .as_ref()
                    .is_none_or(|name| t.agent_name == *name)
            })
            .filter(|t| filter.created_after.is_none_or(|ts| t.created_at >= ts))
            .filter(|t| filter.created_before.is_none_or(|ts| t.created_at <= ts))
            .take(filter.limit.unwrap_or(usize::MAX))
            .collect();
        Ok(filtered)
    }

    /// Count tasks grouped by status.
    pub fn count_by_status(
        &self,
        key: &MasterKey,
    ) -> Result<std::collections::HashMap<TaskStatus, usize>> {
        let tasks = self.list(key)?;
        let mut counts = std::collections::HashMap::new();
        for task in tasks {
            *counts.entry(task.status).or_insert(0) += 1;
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;

    fn temp_store() -> (TaskStore, MasterKey, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("aivyx-task-store-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = TaskStore::open(dir.join("tasks.db")).unwrap();
        let key = MasterKey::from_bytes([42u8; 32]);
        (store, key, dir)
    }

    #[test]
    fn save_and_get_roundtrip() {
        let (store, key, dir) = temp_store();
        let task = Task::new("do something", "agent-1");
        let id = task.id;
        store.save(&task, &key).unwrap();

        let loaded = store.get(&id, &key).unwrap().unwrap();
        assert_eq!(loaded.id, id);
        assert_eq!(loaded.goal, "do something");
        assert_eq!(loaded.agent_name, "agent-1");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn get_missing_returns_none() {
        let (store, key, dir) = temp_store();
        let id = TaskId::new();
        assert!(store.get(&id, &key).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_modifies_and_saves() {
        let (store, key, dir) = temp_store();
        let task = Task::new("update me", "agent");
        let id = task.id;
        store.save(&task, &key).unwrap();

        let updated = store
            .update(&id, &key, |t| {
                t.start();
            })
            .unwrap();
        assert_eq!(updated.status, TaskStatus::Executing);
        assert!(updated.started_at.is_some());

        // Verify persisted
        let loaded = store.get(&id, &key).unwrap().unwrap();
        assert_eq!(loaded.status, TaskStatus::Executing);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn update_missing_task_errors() {
        let (store, key, dir) = temp_store();
        let id = TaskId::new();
        let result = store.update(&id, &key, |_| {});
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_removes_task() {
        let (store, key, dir) = temp_store();
        let task = Task::new("delete me", "agent");
        let id = task.id;
        store.save(&task, &key).unwrap();
        store.delete(&id).unwrap();
        assert!(store.get(&id, &key).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_sorted_newest_first() {
        let (store, key, dir) = temp_store();
        let t1 = Task::new("first", "agent");
        let t2 = Task::new("second", "agent");
        store.save(&t1, &key).unwrap();
        store.save(&t2, &key).unwrap();

        let tasks = store.list(&key).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks[0].created_at >= tasks[1].created_at);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn query_by_status() {
        let (store, key, dir) = temp_store();
        let mut t1 = Task::new("active", "agent");
        t1.start();
        let t2 = Task::new("planning", "agent");
        store.save(&t1, &key).unwrap();
        store.save(&t2, &key).unwrap();

        let filter = TaskFilter {
            status: Some(TaskStatus::Executing),
            ..Default::default()
        };
        let results = store.query(&filter, &key).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].goal, "active");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn query_by_agent() {
        let (store, key, dir) = temp_store();
        store.save(&Task::new("a", "alice"), &key).unwrap();
        store.save(&Task::new("b", "bob"), &key).unwrap();

        let filter = TaskFilter {
            agent_name: Some("bob".into()),
            ..Default::default()
        };
        let results = store.query(&filter, &key).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].agent_name, "bob");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn query_with_limit() {
        let (store, key, dir) = temp_store();
        for i in 0..5 {
            store
                .save(&Task::new(format!("task {i}"), "agent"), &key)
                .unwrap();
        }

        let filter = TaskFilter {
            limit: Some(3),
            ..Default::default()
        };
        let results = store.query(&filter, &key).unwrap();
        assert_eq!(results.len(), 3);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn count_by_status_groups() {
        let (store, key, dir) = temp_store();
        let mut t1 = Task::new("a", "agent");
        t1.start();
        let mut t2 = Task::new("b", "agent");
        t2.complete(None);
        let t3 = Task::new("c", "agent");
        store.save(&t1, &key).unwrap();
        store.save(&t2, &key).unwrap();
        store.save(&t3, &key).unwrap();

        let counts = store.count_by_status(&key).unwrap();
        assert_eq!(counts.get(&TaskStatus::Executing), Some(&1));
        assert_eq!(counts.get(&TaskStatus::Completed), Some(&1));
        assert_eq!(counts.get(&TaskStatus::Planning), Some(&1));

        std::fs::remove_dir_all(&dir).ok();
    }
}
