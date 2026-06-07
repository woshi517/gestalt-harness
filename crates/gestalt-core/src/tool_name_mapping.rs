use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::tool_descriptor::{CanonicalToolId, ToolNamespace};

/// Maximum length of a provider-facing alias. Common provider limits
/// (Anthropic 64, OpenAI 64) sit at or below this value, so we cap here
/// once and let individual providers do their own validation.
pub const MAX_PROVIDER_NAME_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolNameMapping {
    pub internal_id: CanonicalToolId,
    pub provider_name: String,
    pub display_name: String,
    pub descriptor_hash: String,
    /// Provider-strict input schema (with `additionalProperties: false`,
    /// optional fields made nullable, etc.). This is the contract the
    /// provider saw and the contract the executor should validate
    /// against. Optional because some non-strict providers don't
    /// produce one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    /// Whether the provider rendered this tool in strict mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
}

impl ToolNameMapping {
    /// Produce the canonical "base" provider name for a given internal
    /// tool ID. This name is *not* guaranteed to be unique across the
    /// catalog; it is the un-suffixed form that the alias collision
    /// resolver then disambiguates.
    ///
    /// The format mirrors the original (deterministic) contract:
    ///
    /// | Internal ID | Base Provider Name |
    /// | --- | --- |
    /// | `builtin:read` | `read` |
    /// | `extension:mock-ext:convert_pdf` | `ext_mock_ext_convert_pdf` |
    /// | `mcp:brave-search:web_search` | `mcp_brave_search_web_search` |
    pub fn generate_provider_name(internal_id: &CanonicalToolId) -> String {
        let raw = match &internal_id.namespace {
            ToolNamespace::BuiltIn => internal_id.name.clone(),
            ToolNamespace::Extension(id) => {
                let safe_id: String = id
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                let safe_name: String = internal_id
                    .name
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                format!("ext_{}_{}", safe_id, safe_name)
            }
            ToolNamespace::Mcp(id) => {
                let safe_id: String = id
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                let safe_name: String = internal_id
                    .name
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                format!("mcp_{}_{}", safe_id, safe_name)
            }
        };

        let sanitized: String = raw
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();

        if sanitized.len() <= MAX_PROVIDER_NAME_LEN {
            sanitized
        } else {
            sanitized.chars().take(MAX_PROVIDER_NAME_LEN).collect()
        }
    }

    /// Resolve a slice of canonical tool IDs into a deterministic
    /// alias mapping with collision-safe provider names. Output is in
    /// the same order as the input slice so the resulting mapping can
    /// be zipped with its origin list without losing track of identity.
    ///
    /// The disambiguation rule is: keep the first occurrence's base
    /// name unchanged, and append a deterministic `_2`, `_3`, ... suffix
    /// to subsequent collisions. The full canonical ID (after the
    /// namespace prefix) is used to derive the suffix base so the
    /// result is stable across runs and not dependent on iteration
    /// order of a `HashMap`.
    ///
    /// Note: if the same `CanonicalToolId` appears multiple times in
    /// the input, the resolver still treats it as a collision and
    /// disambiguates. In practice a catalog should never contain the
    /// same canonical ID twice — the resolver just defends against
    /// the degenerate case so it never silently collapses two
    /// distinct identities into one alias.
    pub fn resolve_provider_names(internal_ids: &[CanonicalToolId]) -> Vec<(CanonicalToolId, String)> {
        // First, build a count of base-name collisions so we can
        // allocate suffixes deterministically. The order of the input
        // slice defines which tool keeps the un-suffixed name; tools
        // appearing later get `_2`, `_3`, ... in their slice order.
        let mut base_counts: HashMap<String, usize> = HashMap::new();
        for id in internal_ids {
            let base = Self::generate_provider_name(id);
            *base_counts.entry(base).or_insert(0) += 1;
        }

        // Track which base names have been emitted and how many times.
        let mut seen: HashMap<String, usize> = HashMap::new();
        // Track every provider name that has been emitted so we can
        // defend against the (theoretical) case where disambiguation
        // itself collides — e.g. a base that is exactly
        // `MAX_PROVIDER_NAME_LEN - 2` characters long, which can't fit
        // a numeric suffix without truncation.
        let mut used: HashSet<String> = HashSet::new();

        let mut out = Vec::with_capacity(internal_ids.len());
        for id in internal_ids {
            let base = Self::generate_provider_name(id);
            let collision_total = *base_counts.get(&base).unwrap_or(&1);
            let prior_seen = *seen.get(&base).unwrap_or(&0);

            // `occurrence` is 1-based. The first occurrence keeps
            // the bare base; subsequent occurrences get `_2`, `_3`,
            // ... in slice order.
            let occurrence = prior_seen + 1;

            let candidate = if collision_total == 1 || occurrence == 1 {
                base.clone()
            } else {
                let suffix = format!("_{occurrence}");
                truncate_for_suffix(&base, suffix.len()) + &suffix
            };

            // Final defensive uniqueness: if the chosen alias is
            // already used (e.g. via a manual override earlier in the
            // pipeline), keep extending the suffix until it isn't.
            let final_name = if used.contains(&candidate) {
                let mut n = occurrence;
                loop {
                    let suffix = format!("_{n}");
                    let alt = truncate_for_suffix(&base, suffix.len()) + &suffix;
                    if !used.contains(&alt) {
                        break alt;
                    }
                    n += 1;
                }
            } else {
                candidate
            };

            used.insert(final_name.clone());
            *seen.entry(base).or_insert(0) += 1;
            out.push((id.clone(), final_name));
        }

        out
    }

    pub fn new(internal_id: CanonicalToolId, display_name: String, descriptor_hash: String) -> Self {
        // Single-ID construction is best-effort: the caller is opting
        // out of catalog-wide uniqueness guarantees. Downstream code
        // that builds a full mapping should prefer
        // `build_mapping_with_resolution` so collisions are detected
        // and disambiguated.
        let provider_name = Self::generate_provider_name(&internal_id);
        Self {
            internal_id,
            provider_name,
            display_name,
            descriptor_hash,
            input_schema: None,
            strict: None,
        }
    }

    /// Build a mapping for a full catalog with collision-safe provider
    /// names. The order of `tools` defines which colliding tool keeps
    /// the un-suffixed alias, so callers should pass descriptors in a
    /// deterministic order (e.g. sorted by canonical internal ID).
    pub fn build_mapping_with_resolution(
        tools: &[(CanonicalToolId, String, String)],
    ) -> Vec<ToolNameMapping> {
        let ids: Vec<CanonicalToolId> = tools.iter().map(|(id, _, _)| id.clone()).collect();
        let resolved = Self::resolve_provider_names(&ids);
        let mut by_id: HashMap<CanonicalToolId, String> = resolved.into_iter().collect();
        tools
            .iter()
            .map(|(id, display_name, descriptor_hash)| {
                let provider_name = by_id.remove(id).unwrap_or_else(|| Self::generate_provider_name(id));
                ToolNameMapping {
                    internal_id: id.clone(),
                    provider_name,
                    display_name: display_name.clone(),
                    descriptor_hash: descriptor_hash.clone(),
                    input_schema: None,
                    strict: None,
                }
            })
            .collect()
    }
}

/// Truncate `base` so the resulting string plus `suffix_len` chars of
/// suffix still fits within `MAX_PROVIDER_NAME_LEN`. Truncation is
/// performed on a char boundary.
fn truncate_for_suffix(base: &str, suffix_len: usize) -> String {
    let allowed = MAX_PROVIDER_NAME_LEN.saturating_sub(suffix_len);
    if base.chars().count() <= allowed {
        base.to_string()
    } else {
        base.chars().take(allowed).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool_descriptor::{CanonicalToolId, ToolNamespace};

    fn builtin(name: &str) -> CanonicalToolId {
        CanonicalToolId {
            namespace: ToolNamespace::BuiltIn,
            name: name.to_string(),
        }
    }

    fn extension(ext_id: &str, name: &str) -> CanonicalToolId {
        CanonicalToolId {
            namespace: ToolNamespace::Extension(ext_id.to_string()),
            name: name.to_string(),
        }
    }

    #[test]
    fn base_names_match_documented_format() {
        assert_eq!(ToolNameMapping::generate_provider_name(&builtin("read")), "read");
        assert_eq!(
            ToolNameMapping::generate_provider_name(&extension("mock-ext", "convert_pdf")),
            "ext_mock_ext_convert_pdf"
        );
    }

    #[test]
    fn resolver_does_not_disambiguate_when_unique() {
        let ids = vec![builtin("read"), builtin("write")];
        let resolved = ToolNameMapping::resolve_provider_names(&ids);
        assert_eq!(resolved[0].1, "read");
        assert_eq!(resolved[1].1, "write");
    }

    #[test]
    fn resolver_uses_deterministic_suffix_for_collisions() {
        // Two built-ins with the same display name only differ if
        // namespace differs, but two extension tools with the same
        // `name` field and the same extension id will collide.
        let ids = vec![
            extension("ext1", "convert"),
            extension("ext1", "convert"),
            extension("ext1", "convert"),
        ];
        let resolved = ToolNameMapping::resolve_provider_names(&ids);
        assert_eq!(resolved[0].1, "ext_ext1_convert");
        assert_eq!(resolved[1].1, "ext_ext1_convert_2");
        assert_eq!(resolved[2].1, "ext_ext1_convert_3");
    }

    #[test]
    fn resolver_respects_input_order() {
        // Order matters: the first occurrence keeps the bare name.
        let ids = vec![extension("ext1", "convert"), extension("ext1", "convert")];
        let resolved = ToolNameMapping::resolve_provider_names(&ids);
        assert_eq!(resolved[0].1, "ext_ext1_convert");
        assert_eq!(resolved[1].1, "ext_ext1_convert_2");
    }

    #[test]
    fn resolver_truncates_when_suffix_would_overflow_max_length() {
        // Construct a base name that is already at the cap. After
        // appending `_2` the result must still be within
        // MAX_PROVIDER_NAME_LEN.
        let long_name: String = "a".repeat(MAX_PROVIDER_NAME_LEN);
        let id1 = builtin(&long_name);
        let id2 = builtin(&long_name);
        let resolved = ToolNameMapping::resolve_provider_names(&[id1, id2]);
        assert!(resolved[0].1.chars().count() <= MAX_PROVIDER_NAME_LEN);
        assert!(resolved[1].1.chars().count() <= MAX_PROVIDER_NAME_LEN);
        // First occurrence keeps the truncated base, second gets a
        // truncated base plus `_2` and still fits.
        assert_eq!(resolved[0].1, long_name);
        assert_eq!(resolved[1].1.chars().count(), MAX_PROVIDER_NAME_LEN);
        assert!(resolved[1].1.ends_with("_2"));
    }

    #[test]
    fn resolver_emits_distinct_aliases_for_colliding_canonicals() {
        // Two different extension IDs that happen to sanitize to the
        // same provider name (e.g. via punctuation) should still
        // receive distinct aliases.
        let ids = vec![
            CanonicalToolId {
                namespace: ToolNamespace::Extension("ext-one".to_string()),
                name: "convert".to_string(),
            },
            CanonicalToolId {
                namespace: ToolNamespace::Extension("ext_one".to_string()),
                name: "convert".to_string(),
            },
        ];
        let resolved = ToolNameMapping::resolve_provider_names(&ids);
        assert_ne!(resolved[0].1, resolved[1].1);
    }
}
