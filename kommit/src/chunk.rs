//! Cuts a diff into the chunks that 2-step generation summarizes, one request each.

/// Splits `diff` into one diff per file, at each `diff --git` line.
pub(crate) fn split_diff(diff: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut current = String::new();

    for line in diff.lines() {
        if line.starts_with("diff --git ") && !current.is_empty() {
            files.push(current);
            current = String::new();
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        files.push(current);
    }
    files
}

/// Packs the per-file diffs of `files` into as few chunks of at most `max_bytes` as their
/// order allows, so that small files share a request instead of taking one each.
///
/// A file over `max_bytes` is split at its hunks first. Only a piece that still does not
/// fit, a single hunk or a file without hunks, takes a chunk of its own over the limit,
/// left for [`truncate_chunk`] to cut.
pub(crate) fn chunk_files(files: Vec<String>, max_bytes: usize) -> Vec<String> {
    let pieces = files
        .into_iter()
        .flat_map(|file| split_file(file, max_bytes));
    pack(pieces, max_bytes)
}

/// Splits the diff of one file into pieces of at most `max_bytes`, between its hunks.
/// Each piece repeats the file header, so that it still reads as a diff of that file.
fn split_file(file: String, max_bytes: usize) -> Vec<String> {
    if file.len() <= max_bytes {
        return vec![file];
    }

    let mut hunk_starts = Vec::new();
    let mut offset = 0;
    for line in file.split_inclusive('\n') {
        // Lines of a hunk start with ' ', '+', '-' or '\', so only a hunk header starts
        // with "@@".
        if line.starts_with("@@ ") {
            hunk_starts.push(offset);
        }
        offset += line.len();
    }
    let Some(&first_hunk) = hunk_starts.first() else {
        return vec![file];
    };
    let header = &file[..first_hunk];
    if header.len() >= max_bytes {
        return vec![file];
    }
    let room = max_bytes - header.len();

    let hunk_ends = hunk_starts.iter().skip(1).copied().chain([file.len()]);
    let mut pieces = Vec::new();
    let mut hunks = String::new();
    for (start, end) in hunk_starts.iter().copied().zip(hunk_ends) {
        let hunk = &file[start..end];
        if !hunks.is_empty() && hunks.len() + hunk.len() > room {
            pieces.push(format!("{header}{hunks}"));
            hunks.clear();
        }
        hunks.push_str(hunk);
    }
    pieces.push(format!("{header}{hunks}"));
    pieces
}

/// Joins consecutive `pieces` while they fit in `max_bytes`, keeping their order.
fn pack(pieces: impl IntoIterator<Item = String>, max_bytes: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for piece in pieces {
        if !current.is_empty() && current.len() + piece.len() > max_bytes {
            chunks.push(std::mem::take(&mut current));
        }
        current.push_str(&piece);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Cuts `chunk` to `max_bytes` at most, on a character boundary, and says so.
pub(crate) fn truncate_chunk(chunk: String, max_bytes: usize) -> String {
    if chunk.len() <= max_bytes {
        return chunk;
    }
    let end = chunk.floor_char_boundary(max_bytes);
    format!("{}\n...(truncated due to size)...", &chunk[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The diff of `path` with `hunks` hunks, each changing one line to `body`.
    fn file_diff(path: &str, hunks: usize, body: &str) -> String {
        let mut diff = format!(
            "diff --git a/{path} b/{path}\nindex 123456..789abc 100644\n--- a/{path}\n+++ b/{path}\n"
        );
        for i in 0..hunks {
            let line = i * 10 + 1;
            diff.push_str(&format!("@@ -{line} +{line} @@\n-old {i}\n+{body} {i}\n"));
        }
        diff
    }

    fn header_of(path: &str) -> String {
        let diff = file_diff(path, 1, "");
        diff[..diff.find("@@ ").unwrap()].to_string()
    }

    #[test]
    fn test_split_diff_empty() {
        let diff = "";
        let chunks = split_diff(diff);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_split_diff_single() {
        let diff = "diff --git a/src/main.rs b/src/main.rs\nindex 123456..789abc 100644\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,2 +1,2 @@\n-fn main() {}\n+fn main() { println!(\"hello\"); }";
        let chunks = split_diff(diff);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("diff --git a/src/main.rs"));
    }

    #[test]
    fn test_split_diff_multiple() {
        let diff = "diff --git a/file1.rs b/file1.rs\n+ fn first() {}\ndiff --git a/file2.rs b/file2.rs\n+ fn second() {}";
        let chunks = split_diff(diff);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("first"));
        assert!(chunks[1].contains("second"));
    }

    #[test]
    fn no_files_make_no_chunks() {
        assert!(chunk_files(Vec::new(), 100).is_empty());
    }

    #[test]
    fn small_files_share_a_chunk_in_order() {
        let files: Vec<String> = (0..3)
            .map(|i| file_diff(&format!("f{i}"), 1, "new"))
            .collect();
        let chunks = chunk_files(files.clone(), 10_000);
        assert_eq!(chunks, vec![files.concat()]);
    }

    #[test]
    fn a_file_that_does_not_fit_starts_the_next_chunk() {
        let files: Vec<String> = (0..5)
            .map(|i| file_diff(&format!("f{i}"), 1, "new"))
            .collect();
        let max_bytes = files[0].len() * 2;
        let chunks = chunk_files(files.clone(), max_bytes);
        assert_eq!(
            chunks,
            vec![files[..2].concat(), files[2..4].concat(), files[4].clone()]
        );
    }

    #[test]
    fn a_large_file_is_split_between_hunks_with_its_header() {
        let file = file_diff("big", 10, &"x".repeat(50));
        let header = header_of("big");
        let max_bytes = file.len() / 3;
        let chunks = chunk_files(vec![file.clone()], max_bytes);

        assert!(chunks.len() > 1, "{chunks:?}");
        let mut hunks = String::new();
        for chunk in &chunks {
            assert!(chunk.len() <= max_bytes, "{chunk}");
            let body = chunk
                .strip_prefix(&header)
                .expect("each piece has the header");
            assert!(body.starts_with("@@ "), "{chunk}");
            hunks.push_str(body);
        }
        assert_eq!(format!("{header}{hunks}"), file, "no hunk is lost");
    }

    #[test]
    fn the_last_piece_of_a_large_file_shares_a_chunk_with_the_next_file() {
        let big = file_diff("big", 3, &"x".repeat(100));
        let small = file_diff("small", 1, "new");
        let header = header_of("big");
        let hunk = (big.len() - header.len()) / 3;
        // Room for the header and two hunks, then for the last hunk and the small file.
        let max_bytes = header.len() + 2 * hunk;
        assert!(header.len() + hunk + small.len() <= max_bytes);

        let chunks = chunk_files(vec![big, small.clone()], max_bytes);

        assert_eq!(chunks.len(), 2, "{chunks:?}");
        assert!(chunks[1].starts_with(&header), "{}", chunks[1]);
        assert!(chunks[1].ends_with(&small), "{}", chunks[1]);
    }

    #[test]
    fn a_hunk_too_large_for_a_chunk_stays_whole_for_truncation() {
        let small = file_diff("small", 1, "new");
        let big = file_diff("big", 1, &"x".repeat(500));
        let chunks = chunk_files(vec![small.clone(), big.clone(), small.clone()], 300);
        assert_eq!(chunks, vec![small.clone(), big, small]);
    }

    #[test]
    fn a_file_without_hunks_stays_whole_for_truncation() {
        let binary = format!(
            "diff --git a/img.png b/img.png\nindex 123456..789abc 100644\nBinary files a/img.png and b/img.png differ{}\n",
            " ".repeat(200)
        );
        assert_eq!(chunk_files(vec![binary.clone()], 100), vec![binary]);
    }

    #[test]
    fn a_header_too_large_for_a_chunk_keeps_the_file_whole() {
        let file = file_diff("big", 3, "new");
        let max_bytes = header_of("big").len();
        assert_eq!(chunk_files(vec![file.clone()], max_bytes), vec![file]);
    }

    const TRUNCATION_NOTICE: &str = "\n...(truncated due to size)...";
    /// The limit the truncation tests cut at.
    const MAX_CHUNK_BYTES: usize = 12000;

    fn assert_truncated_to(input: String, expected_len: usize) {
        let expected_body = input[..expected_len].to_string();
        let result = truncate_chunk(input, MAX_CHUNK_BYTES);
        let body = result
            .strip_suffix(TRUNCATION_NOTICE)
            .expect("truncated chunk should end with the notice");
        assert_eq!(body.len(), expected_len);
        assert_eq!(body, expected_body);
    }

    // 12,000 is a multiple of 3, so this limit lands on a boundary of '가'.
    #[test]
    fn test_truncate_chunk_multibyte_on_boundary() {
        assert_truncated_to("가".repeat(5000), 12000);
    }

    // Reproduces issue #38: byte 12,000 is inside '가' (bytes 11998..12001).
    #[test]
    fn test_truncate_chunk_multibyte_one_byte_offset() {
        assert_truncated_to(format!("a{}", "가".repeat(5000)), 11998);
    }

    #[test]
    fn test_truncate_chunk_four_byte_char() {
        assert_truncated_to(format!("a{}", "😀".repeat(3000)), 11997);
    }

    #[test]
    fn test_truncate_chunk_ascii_over_limit() {
        assert_truncated_to("a".repeat(12001), 12000);
    }

    #[test]
    fn test_truncate_chunk_at_limit_is_unchanged() {
        let input = "a".repeat(12000);
        assert_eq!(truncate_chunk(input.clone(), MAX_CHUNK_BYTES), input);
    }
}
