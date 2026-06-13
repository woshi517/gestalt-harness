//! Internal backend abstractions for file discovery and text search.
//!
//! These traits define the contract between tool wrappers (find_files, search)
//! and their underlying search implementations. Backend selection is internal
//! to this crate — no user-facing backend config is exposed.

use std::path::PathBuf;

/// A single file match from a file-search backend.
#[derive(Debug, Clone)]
pub struct FileSearchResult {
    /// Absolute path to the matched file.
    pub path: PathBuf,
    /// Backend-assigned relevance score (higher = better match), if available.
    pub score: Option<f64>,
    /// File size in bytes, if available.
    pub file_size: Option<u64>,
    /// Whether this is a directory (true) or file (false).
    pub is_dir: bool,
}

/// Request parameters for a file-search backend.
#[derive(Debug, Clone)]
pub struct FileSearchRequest {
    /// The fuzzy query string to match against file paths.
    pub query: String,
    /// Root directory to search within.
    pub root: PathBuf,
    /// Maximum number of results to return.
    pub max_results: usize,
    /// Optional glob pattern to filter results (e.g., "*.rs").
    pub file_glob: Option<String>,
}

/// Backend trait for file discovery operations.
///
/// Implementations provide fuzzy file-path matching within a directory tree.
/// The tool wrapper handles input validation, path scoping, descendant filtering,
/// and output formatting.
#[async_trait::async_trait]
pub trait FileSearchBackend: Send + Sync {
    /// Returns a human-readable identifier for this backend (e.g., "walkdir").
    fn backend_id(&self) -> &str;

    /// Execute a file search and return matching paths.
    async fn search(&self, request: &FileSearchRequest) -> Result<Vec<FileSearchResult>, BackendError>;
}

/// A single text match from a text-search backend.
#[derive(Debug, Clone)]
pub struct TextSearchResult {
    /// Absolute path to the file containing the match.
    pub path: PathBuf,
    /// 1-based line number of the match.
    pub line_number: usize,
    /// The full content of the matching line.
    pub line_content: String,
    /// Optional context lines before the match.
    pub context_before: Vec<String>,
    /// Optional context lines after the match.
    pub context_after: Vec<String>,
}

/// Request parameters for a text-search backend.
#[derive(Debug, Clone)]
pub struct TextSearchRequest {
    /// The search pattern (literal text or regex).
    pub pattern: String,
    /// Root directory to search within.
    pub root: PathBuf,
    /// Whether `pattern` should be treated as a regex.
    pub is_regex: bool,
    /// Whether the search should be case-insensitive.
    pub case_insensitive: bool,
    /// Number of context lines to include before each match.
    pub context_before: usize,
    /// Number of context lines to include after each match.
    pub context_after: usize,
    /// Maximum number of matching results to return.
    pub max_results: usize,
    /// Optional glob pattern to filter files (e.g., "*.rs").
    pub file_glob: Option<String>,
}

/// Backend trait for text search operations.
///
/// Implementations provide content search within files under a directory tree.
/// The tool wrapper handles input validation, path scoping, descendant filtering,
/// and output formatting.
#[async_trait::async_trait]
pub trait TextSearchBackend: Send + Sync {
    /// Returns a human-readable identifier for this backend (e.g., "ripgrep", "walkdir-grep").
    fn backend_id(&self) -> &str;

    /// Execute a text search and return matching lines.
    async fn search(&self, request: &TextSearchRequest) -> Result<Vec<TextSearchResult>, BackendError>;
}

/// Errors that can occur during backend operations.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The search pattern is invalid (e.g., bad regex).
    #[error("Invalid pattern: {0}")]
    InvalidPattern(String),

    /// An I/O error occurred during search.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// The backend binary is not available (for subprocess-based backends).
    #[error("Backend not available: {0}")]
    NotAvailable(String),

    /// An unexpected backend error.
    #[error("Backend error: {0}")]
    Other(String),
}

// --- Default Backend Constructors ---

/// Create the default file-search backend.
///
/// Currently uses a walkdir-based implementation. This function is the single
/// point of control for backend selection — swap the implementation here
/// when a better backend becomes available.
pub fn default_file_search_backend() -> Box<dyn FileSearchBackend> {
    Box::new(WalkdirFileSearchBackend)
}

/// Create the default text-search backend.
///
/// Currently uses a walkdir+regex grep implementation. This function is the
/// single point of control for backend selection.
pub fn default_text_search_backend() -> Box<dyn TextSearchBackend> {
    Box::new(WalkdirTextSearchBackend)
}

// --- Default Backend Implementations ---

/// File search using walkdir for recursive directory traversal.
///
/// This is a simple baseline implementation. It walks the directory tree,
/// collects matching paths using case-insensitive substring matching against
/// the query, and returns them sorted by match relevance.
struct WalkdirFileSearchBackend;

#[async_trait::async_trait]
impl FileSearchBackend for WalkdirFileSearchBackend {
    fn backend_id(&self) -> &str {
        "walkdir"
    }

    async fn search(&self, request: &FileSearchRequest) -> Result<Vec<FileSearchResult>, BackendError> {
        use walkdir::WalkDir;
        
        let query_lower = request.query.to_lowercase();
        let glob_pattern = request.file_glob.as_ref()
            .map(|g| glob::Pattern::new(g))
            .transpose()
            .map_err(|e| BackendError::InvalidPattern(format!("Invalid glob: {e}")))?;

        let mut results = Vec::new();

        for entry in WalkDir::new(&request.root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            
            // Get the file name for matching
            let file_name = match path.file_name() {
                Some(name) => name.to_string_lossy().to_string(),
                None => continue,
            };

            // Apply glob filter if specified
            if let Some(ref pattern) = glob_pattern {
                if !pattern.matches(&file_name) {
                    continue;
                }
            }

            // Fuzzy match: check if query chars appear in order in the path
            let path_str = path.to_string_lossy().to_lowercase();
            let score = fuzzy_score(&query_lower, &path_str);
            
            if score > 0.0 {
                let metadata = entry.metadata().ok();
                results.push(FileSearchResult {
                    path: path.to_path_buf(),
                    score: Some(score),
                    file_size: metadata.as_ref().map(|m| m.len()),
                    is_dir: entry.file_type().is_dir(),
                });
            }

            if results.len() >= request.max_results * 2 {
                // Collect extra candidates for better ranking, then trim
                break;
            }
        }

        // Sort by score descending, then by path ascending for determinism
        results.sort_by(|a, b| {
            b.score.unwrap_or(0.0)
                .partial_cmp(&a.score.unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });

        results.truncate(request.max_results);
        Ok(results)
    }
}

/// Simple fuzzy scoring: checks if all query characters appear in order
/// in the target string, with bonuses for consecutive matches and
/// matches at word boundaries.
fn fuzzy_score(query: &str, target: &str) -> f64 {
    if query.is_empty() {
        return 1.0; // Empty query matches everything
    }

    let query_chars: Vec<char> = query.chars().collect();
    let target_chars: Vec<char> = target.chars().collect();
    let mut query_idx = 0;
    let mut score = 0.0;
    let mut prev_matched = false;

    for (i, &tc) in target_chars.iter().enumerate() {
        if query_idx < query_chars.len() && tc == query_chars[query_idx] {
            score += 1.0;
            // Bonus for consecutive matches
            if prev_matched {
                score += 0.5;
            }
            // Bonus for matches at path separators or word boundaries
            if i == 0 || target_chars[i - 1] == '/' || target_chars[i - 1] == '_' || target_chars[i - 1] == '-' || target_chars[i - 1] == '.' {
                score += 1.0;
            }
            query_idx += 1;
            prev_matched = true;
        } else {
            prev_matched = false;
        }
    }

    // All query chars must match
    if query_idx < query_chars.len() {
        return 0.0;
    }

    // Normalize by query length so longer queries don't auto-score higher
    score / query_chars.len() as f64
}

/// Text search using walkdir + regex for grep-like content search.
///
/// This replaces the old inline substring search with proper regex support,
/// case sensitivity options, and context lines.
struct WalkdirTextSearchBackend;

#[async_trait::async_trait]
impl TextSearchBackend for WalkdirTextSearchBackend {
    fn backend_id(&self) -> &str {
        "walkdir-grep"
    }

    async fn search(&self, request: &TextSearchRequest) -> Result<Vec<TextSearchResult>, BackendError> {
        use regex::RegexBuilder;
        use walkdir::WalkDir;
        use std::fs;

        // Build the regex pattern
        let pattern = if request.is_regex {
            request.pattern.clone()
        } else {
            regex::escape(&request.pattern)
        };

        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(request.case_insensitive)
            .build()
            .map_err(|e| BackendError::InvalidPattern(format!("Invalid regex: {e}")))?;

        let glob_pattern = request.file_glob.as_ref()
            .map(|g| glob::Pattern::new(g))
            .transpose()
            .map_err(|e| BackendError::InvalidPattern(format!("Invalid glob: {e}")))?;

        let mut results = Vec::new();

        for entry in WalkDir::new(&request.root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if results.len() >= request.max_results {
                break;
            }

            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();

            // Apply glob filter
            if let Some(ref gp) = glob_pattern {
                let file_name = path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                if !gp.matches(&file_name) {
                    continue;
                }
            }

            // Read file content, skip binary files
            let content = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue, // Skip binary/unreadable files
            };

            let lines: Vec<&str> = content.lines().collect();

            for (line_idx, line) in lines.iter().enumerate() {
                if results.len() >= request.max_results {
                    break;
                }

                if regex.is_match(line) {
                    // Collect context lines
                    let context_before: Vec<String> = if request.context_before > 0 {
                        let start = line_idx.saturating_sub(request.context_before);
                        lines[start..line_idx].iter().map(|s| s.to_string()).collect()
                    } else {
                        Vec::new()
                    };

                    let context_after: Vec<String> = if request.context_after > 0 {
                        let end = (line_idx + 1 + request.context_after).min(lines.len());
                        lines[line_idx + 1..end].iter().map(|s| s.to_string()).collect()
                    } else {
                        Vec::new()
                    };

                    results.push(TextSearchResult {
                        path: path.to_path_buf(),
                        line_number: line_idx + 1, // 1-based
                        line_content: line.to_string(),
                        context_before,
                        context_after,
                    });
                }
            }
        }

        // Sort by path ascending, then line number ascending
        results.sort_by(|a, b| {
            a.path.cmp(&b.path)
                .then_with(|| a.line_number.cmp(&b.line_number))
        });

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        // Create test files
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("src/utils")).unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {\n    println!(\"hello\");\n}\n").unwrap();
        fs::write(root.join("src/lib.rs"), "pub mod utils;\n").unwrap();
        fs::write(root.join("src/utils/helpers.rs"), "pub fn helper() -> bool {\n    true\n}\n").unwrap();
        fs::write(root.join("README.md"), "# Test Project\n\nSome content here.\n").unwrap();
        fs::write(root.join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();

        dir
    }

    mod file_search_backend {
        use super::*;

        #[tokio::test]
        async fn returns_walkdir_as_backend_id() {
            let backend = WalkdirFileSearchBackend;
            assert_eq!(backend.backend_id(), "walkdir");
        }

        #[tokio::test]
        async fn finds_files_matching_query() {
            let dir = setup_test_dir();
            let backend = WalkdirFileSearchBackend;
            let request = FileSearchRequest {
                query: "main".to_string(),
                root: dir.path().to_path_buf(),
                max_results: 50,
                file_glob: None,
            };

            let results = backend.search(&request).await.unwrap();
            assert!(!results.is_empty(), "Expected at least one result for 'main'");
            assert!(results.iter().any(|r| r.path.to_string_lossy().contains("main.rs")),
                "Expected main.rs in results");
        }

        #[tokio::test]
        async fn respects_file_glob_filter() {
            let dir = setup_test_dir();
            let backend = WalkdirFileSearchBackend;
            let request = FileSearchRequest {
                query: "".to_string(), // match all
                root: dir.path().to_path_buf(),
                max_results: 50,
                file_glob: Some("*.rs".to_string()),
            };

            let results = backend.search(&request).await.unwrap();
            assert!(results.iter().all(|r| r.path.extension().map_or(false, |e| e == "rs")),
                "All results should be .rs files");
        }

        #[tokio::test]
        async fn respects_max_results() {
            let dir = setup_test_dir();
            let backend = WalkdirFileSearchBackend;
            let request = FileSearchRequest {
                query: "".to_string(),
                root: dir.path().to_path_buf(),
                max_results: 2,
                file_glob: None,
            };

            let results = backend.search(&request).await.unwrap();
            assert!(results.len() <= 2, "Should return at most 2 results");
        }

        #[tokio::test]
        async fn returns_empty_for_no_match() {
            let dir = setup_test_dir();
            let backend = WalkdirFileSearchBackend;
            let request = FileSearchRequest {
                query: "zzzznonexistent".to_string(),
                root: dir.path().to_path_buf(),
                max_results: 50,
                file_glob: None,
            };

            let results = backend.search(&request).await.unwrap();
            assert!(results.is_empty(), "Should return no results for nonexistent query");
        }
    }

    mod text_search_backend {
        use super::*;

        #[tokio::test]
        async fn returns_walkdir_grep_as_backend_id() {
            let backend = WalkdirTextSearchBackend;
            assert_eq!(backend.backend_id(), "walkdir-grep");
        }

        #[tokio::test]
        async fn finds_literal_text_match() {
            let dir = setup_test_dir();
            let backend = WalkdirTextSearchBackend;
            let request = TextSearchRequest {
                pattern: "println".to_string(),
                root: dir.path().to_path_buf(),
                is_regex: false,
                case_insensitive: false,
                context_before: 0,
                context_after: 0,
                max_results: 100,
                file_glob: None,
            };

            let results = backend.search(&request).await.unwrap();
            assert!(!results.is_empty(), "Expected at least one result for 'println'");
            assert!(results[0].line_content.contains("println"));
        }

        #[tokio::test]
        async fn supports_regex_search() {
            let dir = setup_test_dir();
            let backend = WalkdirTextSearchBackend;
            let request = TextSearchRequest {
                pattern: r"fn\s+\w+".to_string(),
                root: dir.path().to_path_buf(),
                is_regex: true,
                case_insensitive: false,
                context_before: 0,
                context_after: 0,
                max_results: 100,
                file_glob: None,
            };

            let results = backend.search(&request).await.unwrap();
            assert!(!results.is_empty(), "Expected regex matches for 'fn <word>'");
        }

        #[tokio::test]
        async fn rejects_invalid_regex() {
            let dir = setup_test_dir();
            let backend = WalkdirTextSearchBackend;
            let request = TextSearchRequest {
                pattern: "[invalid".to_string(),
                root: dir.path().to_path_buf(),
                is_regex: true,
                case_insensitive: false,
                context_before: 0,
                context_after: 0,
                max_results: 100,
                file_glob: None,
            };

            let result = backend.search(&request).await;
            assert!(result.is_err(), "Should reject invalid regex");
            assert!(matches!(result.unwrap_err(), BackendError::InvalidPattern(_)));
        }

        #[tokio::test]
        async fn case_insensitive_search_works() {
            let dir = setup_test_dir();
            let backend = WalkdirTextSearchBackend;
            let request = TextSearchRequest {
                pattern: "PRINTLN".to_string(),
                root: dir.path().to_path_buf(),
                is_regex: false,
                case_insensitive: true,
                context_before: 0,
                context_after: 0,
                max_results: 100,
                file_glob: None,
            };

            let results = backend.search(&request).await.unwrap();
            assert!(!results.is_empty(), "Case-insensitive search should find 'println'");
        }

        #[tokio::test]
        async fn includes_context_lines() {
            let dir = setup_test_dir();
            let backend = WalkdirTextSearchBackend;
            let request = TextSearchRequest {
                pattern: "println".to_string(),
                root: dir.path().to_path_buf(),
                is_regex: false,
                case_insensitive: false,
                context_before: 1,
                context_after: 1,
                max_results: 100,
                file_glob: None,
            };

            let results = backend.search(&request).await.unwrap();
            assert!(!results.is_empty());
            // println is on line 2, so context_before should have line 1
            let result = &results[0];
            assert!(!result.context_before.is_empty(), "Should have context before");
        }

        #[tokio::test]
        async fn respects_file_glob_filter() {
            let dir = setup_test_dir();
            let backend = WalkdirTextSearchBackend;
            let request = TextSearchRequest {
                pattern: "fn".to_string(),
                root: dir.path().to_path_buf(),
                is_regex: false,
                case_insensitive: false,
                context_before: 0,
                context_after: 0,
                max_results: 100,
                file_glob: Some("*.rs".to_string()),
            };

            let results = backend.search(&request).await.unwrap();
            assert!(results.iter().all(|r| r.path.extension().map_or(false, |e| e == "rs")),
                "All results should be from .rs files");
        }

        #[tokio::test]
        async fn results_sorted_by_path_then_line() {
            let dir = setup_test_dir();
            let backend = WalkdirTextSearchBackend;
            let request = TextSearchRequest {
                pattern: "fn".to_string(),
                root: dir.path().to_path_buf(),
                is_regex: false,
                case_insensitive: false,
                context_before: 0,
                context_after: 0,
                max_results: 100,
                file_glob: Some("*.rs".to_string()),
            };

            let results = backend.search(&request).await.unwrap();
            for window in results.windows(2) {
                let ordering = window[0].path.cmp(&window[1].path)
                    .then_with(|| window[0].line_number.cmp(&window[1].line_number));
                assert!(ordering != std::cmp::Ordering::Greater,
                    "Results should be sorted by path then line number");
            }
        }

        #[tokio::test]
        async fn respects_max_results() {
            let dir = setup_test_dir();
            let backend = WalkdirTextSearchBackend;
            let request = TextSearchRequest {
                pattern: "".to_string(), // match every line
                root: dir.path().to_path_buf(),
                is_regex: false,
                case_insensitive: false,
                context_before: 0,
                context_after: 0,
                max_results: 3,
                file_glob: None,
            };

            let results = backend.search(&request).await.unwrap();
            assert!(results.len() <= 3, "Should respect max_results limit");
        }
    }

    mod fuzzy_scoring {
        use super::*;

        #[test]
        fn empty_query_matches_everything() {
            assert!(fuzzy_score("", "anything") > 0.0);
        }

        #[test]
        fn exact_substring_scores_high() {
            let score = fuzzy_score("main", "src/main.rs");
            assert!(score > 0.0, "Should match 'main' in 'src/main.rs'");
        }

        #[test]
        fn nonexistent_chars_score_zero() {
            assert_eq!(fuzzy_score("zzz", "abc"), 0.0);
        }

        #[test]
        fn word_boundary_scores_higher() {
            let boundary_score = fuzzy_score("m", "src/main.rs");
            let mid_score = fuzzy_score("a", "src/main.rs");
            // 'm' at path boundary (after /) should score higher than 'a' mid-word
            assert!(boundary_score >= mid_score,
                "Word boundary match should score >= mid-word match");
        }
    }
}
