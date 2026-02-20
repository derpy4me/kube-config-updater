use std::collections::HashMap;
use std::path::Path;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const STATE_FILE: &str = "/tmp/kube_config_updater_state.json";
const STATE_FILE_TMP: &str = "/tmp/kube_config_updater_state.json.tmp";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ServerRunState {
    pub status: RunStatus,
    pub last_updated: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum RunStatus {
    Fetched,
    Skipped,
    NoCredential,
    AuthRejected,
    Failed,
}

/// Read state file. Returns empty map if file does not exist.
pub fn read_state() -> Result<HashMap<String, ServerRunState>, anyhow::Error> {
    let path = Path::new(STATE_FILE);
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let content = std::fs::read_to_string(path)?;
    let map = serde_json::from_str(&content)?;
    Ok(map)
}

/// Write state file atomically (write to .tmp then rename).
pub fn write_state(states: &HashMap<String, ServerRunState>) -> Result<(), anyhow::Error> {
    let json = serde_json::to_string_pretty(states)?;
    std::fs::write(STATE_FILE_TMP, &json)?;
    std::fs::rename(STATE_FILE_TMP, STATE_FILE)?;
    Ok(())
}

/// Read the current state, update one entry, write back.
pub fn update_server_state(name: &str, state: ServerRunState) -> Result<(), anyhow::Error> {
    let mut states = read_state()?;
    states.insert(name.to_string(), state);
    write_state(&states)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Mutex;

    // Serialize all state-file tests — they share /tmp/kube_config_updater_state.json
    static STATE_FILE_LOCK: Mutex<()> = Mutex::new(());

    fn make_state(status: RunStatus) -> ServerRunState {
        ServerRunState {
            status,
            last_updated: Some(Utc::now()),
            error: None,
        }
    }

    #[test]
    fn test_read_state_missing_file_returns_empty() {
        let _guard = STATE_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Remove the state file so we get a clean "missing" state
        let _ = std::fs::remove_file(STATE_FILE);
        let result = read_state();
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_write_and_read_roundtrip() {
        let _guard = STATE_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut states = HashMap::new();
        states.insert("server1".to_string(), make_state(RunStatus::Fetched));
        states.insert("server2".to_string(), ServerRunState {
            status: RunStatus::Failed,
            last_updated: Some(Utc::now()),
            error: Some("Connection refused".to_string()),
        });

        write_state(&states).expect("write should succeed");
        let loaded = read_state().expect("read should succeed");

        assert_eq!(loaded.len(), 2);
        assert!(matches!(loaded["server1"].status, RunStatus::Fetched));
        assert!(matches!(loaded["server2"].status, RunStatus::Failed));
        assert_eq!(loaded["server2"].error.as_deref(), Some("Connection refused"));
    }

    #[test]
    fn test_update_server_state_merges() {
        let _guard = STATE_FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut initial = HashMap::new();
        initial.insert("existing".to_string(), make_state(RunStatus::Skipped));
        write_state(&initial).expect("write should succeed");

        // Update should add server2 without removing server1
        update_server_state("new_server", make_state(RunStatus::Fetched))
            .expect("update should succeed");

        let loaded = read_state().expect("read should succeed");
        assert!(loaded.contains_key("existing"));
        assert!(loaded.contains_key("new_server"));
    }
}
