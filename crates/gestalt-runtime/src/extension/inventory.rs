use super::ResolvedExtensionPackage;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExtensionInventory {
    packages: Vec<ResolvedExtensionPackage>,
}

impl ExtensionInventory {
    pub fn new(packages: Vec<ResolvedExtensionPackage>) -> Self {
        Self { packages }
    }

    pub fn add_package(&mut self, package: ResolvedExtensionPackage) {
        self.packages.push(package);
    }

    pub fn packages(&self) -> &[ResolvedExtensionPackage] {
        &self.packages
    }

    pub fn find_package(&self, package_id: &str) -> Option<&ResolvedExtensionPackage> {
        self.packages
            .iter()
            .find(|package| package.descriptor.id == package_id)
    }
}
