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

#[test]
fn docs_conformance_matrix_covers_published_contracts() {
    let matrix = fs::read_to_string(repository_root().join("docs/v0.1/conformance-matrix.md"))
        .expect("read v0.1 conformance matrix");
    let expected = [
        "RUNTIME-001",
        "RUNTIME-002",
        "RUNTIME-003",
        "RUNTIME-004",
        "RUNTIME-005",
        "RUNTIME-006",
        "APP-001",
        "APP-002",
        "CTX-001",
        "CTX-002",
        "CTX-003",
        "CTX-004",
        "CFG-001",
        "CFG-002",
        "CFG-003",
        "CFG-004",
        "CFG-005",
        "EXT-001",
        "EXT-002",
        "EXT-003",
        "EXT-004",
        "EXT-005",
        "EVT-001",
        "EVT-002",
        "EVT-003",
        "EVT-004",
        "EVT-005",
        "POL-001",
        "POL-002",
        "POL-003",
        "POL-004",
        "CLI-001",
        "CLI-002",
        "CLI-003",
        "CLI-004",
    ];

    for contract_id in expected {
        let prefix = format!("| {contract_id} |");
        let rows: Vec<_> = matrix
            .lines()
            .filter(|line| line.starts_with(&prefix))
            .collect();
        assert_eq!(rows.len(), 1, "{contract_id} must have exactly one row");
        let columns: Vec<_> = rows[0].split('|').map(str::trim).collect();
        assert_eq!(columns.len(), 9, "{contract_id} has malformed columns");
        assert!(!columns[4].is_empty(), "{contract_id} lacks implementation");
        assert!(
            !columns[5].is_empty(),
            "{contract_id} lacks enforcing tests"
        );
        assert_eq!(columns[6], "published", "{contract_id} status drifted");
    }
}
