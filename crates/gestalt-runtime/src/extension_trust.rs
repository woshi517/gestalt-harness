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
