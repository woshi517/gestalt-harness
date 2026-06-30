#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct TrustedExtensionPin {
    pub package_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_hash: Option<String>,
}

impl TrustedExtensionPin {
    pub fn new(package_id: impl Into<String>, manifest_hash: Option<String>) -> Self {
        Self {
            package_id: package_id.into(),
            manifest_hash,
        }
    }

    pub fn from_config_entry(entry: &str, discovered_manifest_hash: Option<&str>) -> Self {
        if let Some((package_id, manifest_hash)) = entry.split_once(':') {
            let manifest_hash = manifest_hash.trim();
            return Self::new(
                package_id.trim(),
                if manifest_hash.is_empty() {
                    None
                } else {
                    Some(manifest_hash.to_string())
                },
            );
        }

        Self::new(
            entry.trim(),
            discovered_manifest_hash.map(|hash| hash.to_string()),
        )
    }

    pub fn matches(&self, package_id: &str, manifest_hash: Option<&str>) -> bool {
        if self.package_id != package_id {
            return false;
        }

        match (self.manifest_hash.as_deref(), manifest_hash) {
            (Some(expected), Some(actual)) => expected == actual,
            (None, None) => true,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExtensionTrust {
    BuiltIn,
    IntegrityTrusted { manifest_hash: String },
    Untrusted,
}

impl ExtensionTrust {
    pub fn is_trusted(&self) -> bool {
        matches!(self, Self::BuiltIn | Self::IntegrityTrusted { .. })
    }
}
