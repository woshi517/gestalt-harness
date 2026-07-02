use std::fs;
use std::path::{Path, PathBuf};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("runtime crate must be under the repository crates directory")
        .to_path_buf()
}

fn markdown_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("read documentation directory") {
        let path = entry.expect("read documentation entry").path();
        if path.is_dir() {
            markdown_files(&path, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("md") {
            files.push(path);
        }
    }
}

fn active_guides(root: &Path) -> Vec<PathBuf> {
    [
        "docs/composition-hooks-guide.md",
        "docs/extension-development-guide.md",
        "docs/extension-manifest-schema.md",
        "docs/jsonrpc-extension-protocol.md",
        "docs/mcp-client-best-practices.md",
        "docs/permissions-model.md",
        "docs/runtime-event-bus.md",
        "docs/release-checklist.md",
        "docs/skill-specification.md",
        "docs/tui-design.md",
    ]
    .into_iter()
    .map(|path| root.join(path))
    .collect()
}

#[test]
fn docs_links_valid() {
    let root = repository_root();
    let docs = root.join("docs");
    let mut files = Vec::new();
    markdown_files(&docs, &mut files);

    for file in files {
        let source = fs::read_to_string(&file).expect("read Markdown file");
        let mut fenced = false;
        for line in source.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fenced = !fenced;
                continue;
            }
            if fenced {
                continue;
            }

            let mut remaining = line;
            while let Some(start) = remaining.find("](") {
                remaining = &remaining[start + 2..];
                let Some(end) = remaining.find(')') else {
                    break;
                };
                let raw_target = remaining[..end].trim();
                remaining = &remaining[end + 1..];
                let target = raw_target
                    .strip_prefix('<')
                    .and_then(|value| value.strip_suffix('>'))
                    .unwrap_or(raw_target);
                let target = target.split('#').next().unwrap_or_default();
                if target.is_empty()
                    || target.starts_with('/')
                    || target.starts_with("http://")
                    || target.starts_with("https://")
                    || target.starts_with("mailto:")
                    || target.starts_with("file://")
                {
                    continue;
                }
                let resolved = file
                    .parent()
                    .expect("Markdown file has parent")
                    .join(target);
                assert!(
                    resolved.exists(),
                    "broken Markdown link in {}: {target}",
                    file.display()
                );
            }
        }
    }
}

#[test]
fn docs_no_legacy_toml_in_active_guides() {
    let root = repository_root();
    let banned = [
        "config.toml",
        "secret:",
        "gestalt_trace",
        "gestalt_tools",
        "gestalt_context",
        "gestalt_policy",
    ];
    for file in active_guides(&root) {
        let source = fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("read {}: {error}", file.display()))
            .to_ascii_lowercase();
        for pattern in banned {
            assert!(
                !source.contains(pattern),
                "{} contains stale active guidance: {pattern}",
                file.display()
            );
        }
    }
}

#[test]
fn docs_no_v1_extension_in_active_guides() {
    let root = repository_root();
    let banned = [
        "manifest_version = 1",
        "protocol v1 remains",
        "json-rpc 1.0",
        "v1 fallback",
        "fallback to v1",
    ];
    for file in active_guides(&root) {
        let source = fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("read {}: {error}", file.display()))
            .to_ascii_lowercase();
        for pattern in banned {
            assert!(
                !source.contains(pattern),
                "{} contains stale active extension guidance: {pattern}",
                file.display()
            );
        }
    }
}

#[test]
fn docs_v01_published_contracts_have_tests() {
    let index = fs::read_to_string(repository_root().join("docs/v0.1/README.md"))
        .expect("read v0.1 contract index");
    for section in index.split("\n## ").skip(1) {
        if section.contains("**Published**") {
            assert!(
                section.contains("**Enforcing Tests:**"),
                "published section lacks enforcing tests: {}",
                section.lines().next().unwrap_or("<untitled>")
            );
        }
    }
}

#[test]
fn docs_contract_map_matches_status() {
    let root = repository_root();
    let index =
        fs::read_to_string(root.join("docs/v0.1/README.md")).expect("read v0.1 contract index");
    let inventory = fs::read_to_string(root.join("docs/v0.1/contract-inventory.md"))
        .expect("read v0.1 contract inventory");
    let published_inventory = inventory
        .split("## Experimental and Internal Rust Surface")
        .next()
        .expect("published inventory section");
    let contracts = [
        "embedding-control.md",
        "runtime-api.md",
        "app-services.md",
        "context-build-report.md",
        "configuration.md",
        "trace-events.md",
        "policy-approval.md",
        "extensions.md",
        "cli-automation.md",
    ];

    for contract in contracts {
        let source = fs::read_to_string(root.join("docs/v0.1").join(contract))
            .unwrap_or_else(|error| panic!("read {contract}: {error}"));
        assert!(
            source.contains("\nstatus: published\n"),
            "{contract} is not marked published"
        );
        assert!(
            index.contains(&format!("(./{contract})")),
            "{contract} is absent from the v0.1 index"
        );
        assert!(
            published_inventory.contains(&format!("(./{contract})")),
            "{contract} is absent from the published contract inventory"
        );
    }
}
