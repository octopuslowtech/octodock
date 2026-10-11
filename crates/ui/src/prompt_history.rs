use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "prompt-history.json";
pub const PROMPT_HISTORY_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptEntry {
    pub text: String,
    pub sent_at: i64,
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PromptHistory(HashMap<String, Vec<PromptEntry>>);

impl PromptHistory {
    pub fn load(data_dir: &Path) -> Self {
        match std::fs::read_to_string(Self::path(data_dir)) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|err| {
                tracing::warn!(error = %err, "prompt-history corrupt; starting empty");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, data_dir: &Path) -> io::Result<()> {
        std::fs::create_dir_all(data_dir)?;
        let path = Self::path(data_dir);
        let tmp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let result = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, &path));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join(FILE_NAME)
    }

    pub fn entries(&self, project: &str) -> &[PromptEntry] {
        self.0.get(project).map(Vec::as_slice).unwrap_or_default()
    }

    pub fn record(&mut self, project: &str, text: &str, sent_at: i64) {
        if text.trim().is_empty() {
            return;
        }
        let entries = self.0.entry(project.to_string()).or_default();
        entries.retain(|entry| entry.text != text);
        entries.insert(
            0,
            PromptEntry {
                text: text.to_string(),
                sent_at,
            },
        );
        entries.truncate(PROMPT_HISTORY_LIMIT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_newest_first_per_project_without_duplicates_and_caps() {
        let mut history = PromptHistory::default();
        history.record("a", "first", 1);
        history.record("a", "second", 2);
        history.record("b", "other", 3);
        history.record("a", "  ", 4);
        history.record("a", "first", 5);
        let texts: Vec<_> = history.entries("a").iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["first", "second"]);
        assert_eq!(history.entries("a")[0].sent_at, 5);
        assert_eq!(history.entries("b").len(), 1);
        assert!(history.entries("missing").is_empty());

        for ix in 0..(PROMPT_HISTORY_LIMIT as i64 + 10) {
            history.record("a", &format!("prompt {ix}"), 10 + ix);
        }
        let entries = history.entries("a");
        assert_eq!(entries.len(), PROMPT_HISTORY_LIMIT);
        assert_eq!(entries[0].text, "prompt 59");
        assert!(entries.windows(2).all(|pair| pair[0].sent_at > pair[1].sent_at));
    }

    #[test]
    fn round_trips_through_disk_and_survives_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let mut history = PromptHistory::default();
        history.record("a", "hello", 7);
        history.save(dir.path()).unwrap();
        assert_eq!(PromptHistory::load(dir.path()), history);

        std::fs::write(PromptHistory::path(dir.path()), "not json").unwrap();
        assert_eq!(PromptHistory::load(dir.path()), PromptHistory::default());
    }
}
