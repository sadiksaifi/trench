/// Sanitize a branch name for use as a filesystem directory name.
///
/// Rules (FR-15, FR-16):
/// - `/` → `-`
/// - spaces → `-`
/// - `@` → `-`
/// - `..` → stripped
/// - consecutive dashes collapsed
/// - single dots preserved
pub fn sanitize_branch(branch: &str) -> String {
    // Replace `..` sequences (path traversal) with dash
    let stripped = branch.replace("..", "-");

    let mut result = String::with_capacity(stripped.len());
    for ch in stripped.chars() {
        match ch {
            '/' | '@' | ' ' => {
                // Replace with dash, but avoid consecutive dashes
                if !result.ends_with('-') {
                    result.push('-');
                }
            }
            '-' => {
                if !result.ends_with('-') {
                    result.push('-');
                }
            }
            _ => result.push(ch),
        }
    }

    // Trim leading/trailing dashes
    result.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_slash_to_dash() {
        assert_eq!(sanitize_branch("feature/auth"), "feature-auth");
    }

    #[test]
    fn sanitize_at_to_dash() {
        assert_eq!(sanitize_branch("fix@home"), "fix-home");
    }

    #[test]
    fn sanitize_double_dots_stripped() {
        assert_eq!(sanitize_branch("a..b"), "a-b");
    }

    #[test]
    fn sanitize_consecutive_dashes_collapsed() {
        assert_eq!(sanitize_branch("a--b"), "a-b");
    }

    #[test]
    fn sanitize_single_dots_preserved() {
        assert_eq!(sanitize_branch("v2.1.3"), "v2.1.3");
    }

    #[test]
    fn sanitize_spaces_to_dash() {
        assert_eq!(sanitize_branch("my branch"), "my-branch");
    }

    #[test]
    fn sanitize_leading_trailing_dashes_trimmed() {
        assert_eq!(sanitize_branch("/leading"), "leading");
        assert_eq!(sanitize_branch("trailing/"), "trailing");
    }

    #[test]
    fn sanitize_combined_edge_cases() {
        // Multiple replaceable chars in a row collapse to single dash
        assert_eq!(sanitize_branch("a/@b"), "a-b");
        // Empty after stripping
        assert_eq!(sanitize_branch(".."), "");
        // Nested double dots with other chars
        assert_eq!(sanitize_branch("feature/..secret/auth"), "feature-secret-auth");
    }
}
