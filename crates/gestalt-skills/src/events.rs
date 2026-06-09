use serde::{Deserialize, Serialize};

/// Lifecycle events for the skill system.
///
/// These are emitted by the runtime event bus and included in traces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillEvent {
    Discovered {
        skill_name: String,
        manifest_hash: String,
        source: String,
        trust_level: String,
    },
    Activated {
        skill_name: String,
        manifest_hash: String,
        reason: ActivationReason,
    },
    Deactivated {
        skill_name: String,
        manifest_hash: String,
    },
    Rejected {
        skill_name: String,
        reason: String,
    },
    PolicyApplied {
        skill_name: String,
        allowed_tools: Vec<String>,
    },
    ResourceAccessed {
        skill_name: String,
        resource_path: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ActivationReason {
    ExplicitUser,
    CliProvided,
    TriggerMatch,
}

impl SkillEvent {
    pub fn discovered(skill_name: impl Into<String>, manifest_hash: impl Into<String>, source: impl Into<String>, trust_level: impl Into<String>) -> Self {
        Self::Discovered {
            skill_name: skill_name.into(),
            manifest_hash: manifest_hash.into(),
            source: source.into(),
            trust_level: trust_level.into(),
        }
    }

    pub fn activated(skill_name: impl Into<String>, manifest_hash: impl Into<String>, reason: ActivationReason) -> Self {
        Self::Activated {
            skill_name: skill_name.into(),
            manifest_hash: manifest_hash.into(),
            reason,
        }
    }

    pub fn deactivated(skill_name: impl Into<String>, manifest_hash: impl Into<String>) -> Self {
        Self::Deactivated {
            skill_name: skill_name.into(),
            manifest_hash: manifest_hash.into(),
        }
    }

    pub fn rejected(skill_name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Rejected {
            skill_name: skill_name.into(),
            reason: reason.into(),
        }
    }

    pub fn policy_applied(skill_name: impl Into<String>, allowed_tools: Vec<String>) -> Self {
        Self::PolicyApplied {
            skill_name: skill_name.into(),
            allowed_tools,
        }
    }

    pub fn resource_accessed(skill_name: impl Into<String>, resource_path: impl Into<String>) -> Self {
        Self::ResourceAccessed {
            skill_name: skill_name.into(),
            resource_path: resource_path.into(),
        }
    }
}
