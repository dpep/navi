//! Small shared helpers: content hashing, language/extension mapping, snippet
//! trimming, range parsing.

use sha2::{Digest, Sha256};

use crate::error::{NaviError, Result};

const SNIPPET_MAX: usize = 200;

/// Parse a 1-based inclusive line range against a file of `total` lines.
/// Forms: "A:B" (closed), "A:" (A to end), ":B" (start to B), "A" (line A).
/// A missing end resolves to 1 (start) or `total` (end); an explicit end is
/// returned as-is so callers keep their own clamping/bounds behavior.
pub fn parse_range(s: &str, total: usize) -> Result<(usize, usize)> {
    let (a, b) = match s.trim().split_once(':') {
        Some((start, end)) => (range_end(start, 1)?, range_end(end, total)?),
        None => {
            let n = range_num(s)?;
            (n, n)
        }
    };
    if a < 1 || b < a {
        return Err(NaviError::new(
            "invalid_args",
            "range must satisfy 1 <= A <= B",
        ));
    }
    Ok((a, b))
}

/// One side of a range: empty falls back to `default`, else a line number.
fn range_end(s: &str, default: usize) -> Result<usize> {
    if s.trim().is_empty() {
        Ok(default)
    } else {
        range_num(s)
    }
}

fn range_num(s: &str) -> Result<usize> {
    s.trim().parse().map_err(|_| {
        NaviError::new(
            "invalid_args",
            "range must be A:B, A:, :B, or A (line numbers), e.g. 40:80",
        )
    })
}

pub fn content_hash(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

/// Trim a line to a single-line, length-capped snippet.
pub fn snippet(line: &str) -> String {
    let trimmed = line.trim_end_matches(['\n', '\r']).trim();
    if trimmed.chars().count() > SNIPPET_MAX {
        let cut: String = trimmed.chars().take(SNIPPET_MAX).collect();
        format!("{cut}…")
    } else {
        trimmed.to_string()
    }
}

/// Map a language name to a file glob (for `rg -g` / `find -name`). Returns
/// None for languages we don't recognize, in which case no filter is applied.
pub fn lang_glob(lang: &str) -> Option<String> {
    lang_ext(lang).map(|ext| format!("*.{ext}"))
}

pub fn lang_ext(lang: &str) -> Option<&'static str> {
    let ext = match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rs",
        "go" | "golang" => "go",
        "python" | "py" => "py",
        "ruby" | "rb" => "rb",
        "javascript" | "js" => "js",
        "typescript" | "ts" => "ts",
        "java" => "java",
        "c" => "c",
        "cpp" | "c++" | "cxx" => "cpp",
        "csharp" | "cs" => "cs",
        "php" => "php",
        "swift" => "swift",
        "kotlin" | "kt" => "kt",
        "scala" => "scala",
        "shell" | "bash" | "sh" => "sh",
        _ => return None,
    };
    Some(ext)
}

/// Best-effort language label from a path's extension (for `read` output).
pub fn lang_from_path(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    let lang = match ext {
        "rs" => "rust",
        "go" => "go",
        "py" => "python",
        "rb" => "ruby",
        "js" => "javascript",
        "ts" => "typescript",
        "java" => "java",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "swift" => "swift",
        "kt" => "kotlin",
        "scala" => "scala",
        "sh" | "bash" => "shell",
        _ => return None,
    };
    Some(lang)
}
