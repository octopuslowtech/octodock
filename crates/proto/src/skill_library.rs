use serde::{Deserialize, Serialize};

use crate::HarnessId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillSyncMode {
    #[default]
    Symlink,
    Copy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SkillSource {
    Local {
        path: String,
    },
    Archive {
        path: String,
    },
    Git {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillDeploymentState {
    Deployed,
    Missing,
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillDeployment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessId>,
    pub mode: SkillSyncMode,
    pub state: SkillDeploymentState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibrarySkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: SkillSource,
    pub update_available: bool,
    pub deployments: Vec<SkillDeployment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessId>,
    pub path: String,
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedSkill {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessId>,
    pub name: String,
    pub description: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillPreset {
    pub id: String,
    pub name: String,
    pub skills: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillLibraryView {
    pub mode: SkillSyncMode,
    pub skills: Vec<LibrarySkill>,
    pub targets: Vec<SkillTarget>,
    pub unmanaged: Vec<UnmanagedSkill>,
    pub presets: Vec<SkillPreset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum SkillLibraryOp {
    InstallPath {
        path: String,
    },
    InstallGit {
        source: String,
    },
    Remove {
        skill: String,
    },
    Deploy {
        skill: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        harness: Option<HarnessId>,
    },
    Undeploy {
        skill: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        harness: Option<HarnessId>,
    },
    Adopt {
        path: String,
    },
    CheckUpdates,
    Update {
        skill: String,
    },
    SavePreset {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        name: String,
        skills: Vec<String>,
    },
    DeletePreset {
        id: String,
    },
    ApplyPreset {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        harness: Option<HarnessId>,
        enabled: bool,
    },
    SetMode {
        mode: SkillSyncMode,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_round_trip_with_a_flattened_scope() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Params {
            #[serde(default)]
            space_id: Option<String>,
            #[serde(flatten)]
            op: SkillLibraryOp,
        }
        let params = Params {
            space_id: Some("space".into()),
            op: SkillLibraryOp::ApplyPreset {
                id: "preset".into(),
                harness: Some(HarnessId::ClaudeCode),
                enabled: true,
            },
        };
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "spaceId": "space",
                "op": "applyPreset",
                "id": "preset",
                "harness": "claude-code",
                "enabled": true,
            })
        );
        assert_eq!(serde_json::from_value::<Params>(value).unwrap(), params);
        let shared: Params =
            serde_json::from_value(serde_json::json!({"op": "deploy", "skill": "review"})).unwrap();
        assert_eq!(
            shared.op,
            SkillLibraryOp::Deploy {
                skill: "review".into(),
                harness: None,
            }
        );
    }
}
