use crate::error::{Result, RuntimeError};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

pub trait ArtifactStore: Send + Sync {
    fn put_artifact(&self, session_id: &str, name: &str, content: &[u8]) -> Result<String>;
    fn get_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>>;
    fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>>;
}

pub struct InMemoryArtifactStore {
    artifacts: Mutex<HashMap<String, HashMap<String, Vec<u8>>>>,
}

impl InMemoryArtifactStore {
    pub fn new() -> Self {
        Self {
            artifacts: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryArtifactStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ArtifactStore for InMemoryArtifactStore {
    fn put_artifact(&self, session_id: &str, name: &str, content: &[u8]) -> Result<String> {
        let mut guard = self.artifacts.lock().unwrap();
        let session_map = guard.entry(session_id.to_string()).or_default();
        session_map.insert(name.to_string(), content.to_vec());
        Ok(format!("memory://{}/{}", session_id, name))
    }

    fn get_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>> {
        let guard = self.artifacts.lock().unwrap();
        let content = guard
            .get(session_id)
            .and_then(|session_map| session_map.get(name))
            .ok_or_else(|| {
                RuntimeError::Orchestration(format!(
                    "Artifact not found: {} for session {}",
                    name, session_id
                ))
            })?;
        Ok(content.clone())
    }

    fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>> {
        let guard = self.artifacts.lock().unwrap();
        let list = guard
            .get(session_id)
            .map(|session_map| session_map.keys().cloned().collect())
            .unwrap_or_default();
        Ok(list)
    }
}

pub struct FilesystemArtifactStore {
    base_dir: PathBuf,
}

impl FilesystemArtifactStore {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    fn get_session_dir(&self, session_id: &str) -> Result<PathBuf> {
        if session_id.contains("..") || session_id.contains('/') || session_id.contains('\\') {
            return Err(RuntimeError::Orchestration(
                "Invalid session ID".to_string(),
            ));
        }
        let dir = self.base_dir.join(session_id);
        Ok(dir)
    }

    fn get_artifact_path(&self, session_id: &str, name: &str) -> Result<PathBuf> {
        if name.contains("..") || name.contains('/') || name.contains('\\') {
            return Err(RuntimeError::Orchestration(
                "Invalid artifact name".to_string(),
            ));
        }
        let dir = self.get_session_dir(session_id)?;
        Ok(dir.join(name))
    }
}

impl ArtifactStore for FilesystemArtifactStore {
    fn put_artifact(&self, session_id: &str, name: &str, content: &[u8]) -> Result<String> {
        let path = self.get_artifact_path(session_id, name)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| RuntimeError::Orchestration(e.to_string()))?;
        }
        fs::write(&path, content).map_err(|e| RuntimeError::Orchestration(e.to_string()))?;
        Ok(path.to_string_lossy().to_string())
    }

    fn get_artifact(&self, session_id: &str, name: &str) -> Result<Vec<u8>> {
        let path = self.get_artifact_path(session_id, name)?;
        fs::read(&path).map_err(|e| RuntimeError::Orchestration(e.to_string()))
    }

    fn list_artifacts(&self, session_id: &str) -> Result<Vec<String>> {
        let dir = self.get_session_dir(session_id)?;
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut list = Vec::new();
        for entry in fs::read_dir(dir).map_err(|e| RuntimeError::Orchestration(e.to_string()))? {
            let entry = entry.map_err(|e| RuntimeError::Orchestration(e.to_string()))?;
            if entry
                .file_type()
                .map_err(|e| RuntimeError::Orchestration(e.to_string()))?
                .is_file()
            {
                list.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        Ok(list)
    }
}
