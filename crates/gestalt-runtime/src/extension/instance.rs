#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ComponentInstanceId {
    pub package_id: String,
    pub instance_id: String,
    pub component_id: String,
}

impl ComponentInstanceId {
    pub fn new(
        package_id: impl Into<String>,
        instance_id: impl Into<String>,
        component_id: impl Into<String>,
    ) -> Self {
        Self {
            package_id: package_id.into(),
            instance_id: instance_id.into(),
            component_id: component_id.into(),
        }
    }

    pub fn canonical_id(&self) -> String {
        format!(
            "component:{}:{}:{}",
            self.package_id, self.instance_id, self.component_id
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionInstanceSpec {
    pub instance_id: String,
    pub package_id: String,
}

impl ExtensionInstanceSpec {
    pub fn canonical_id(&self) -> String {
        format!("instance:{}", self.instance_id)
    }
}
