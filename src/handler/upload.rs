use crate::util::find_subslice;

/// A single file extracted from a `multipart/form-data` body.
pub struct UploadedFile {
    pub filename: String,
    pub data: Vec<u8>,
}

/// Extracts the `boundary=` parameter from a `Content-Type` header value.
pub fn extract_boundary(content_type: &str) -> Option<String> {
    for part in content_type.split(';') {
        let part = part.trim();
        if let Some(b) = part.strip_prefix("boundary=") {
            return Some(b.trim_matches('"').to_string());
        }
    }
    None
}

/// Parses a `multipart/form-data` body into its constituent file parts.
/// Form fields without a `filename` are ignored.
pub fn parse_multipart(body: &[u8], boundary: &str) -> Vec<UploadedFile> {
    let delim = format!("--{}", boundary);
    let delim_bytes = delim.as_bytes();
    let mut result = Vec::new();

    let mut positions = Vec::new();
    let mut search_from = 0;
    while search_from <= body.len() {
        match find_subslice(&body[search_from..], delim_bytes) {
            Some(pos) => {
                positions.push(search_from + pos);
                search_from = search_from + pos + delim_bytes.len();
            }
            None => break,
        }
    }

    for i in 0..positions.len() {
        let start = positions[i] + delim_bytes.len();
        let end = if i + 1 < positions.len() {
            positions[i + 1]
        } else {
            body.len()
        };
        if start >= end {
            continue;
        }
        let mut chunk = &body[start..end];
        if chunk.starts_with(b"--") {
            continue; // final boundary marker
        }
        if chunk.starts_with(b"\r\n") {
            chunk = &chunk[2..];
        }
        let header_end = match find_subslice(chunk, b"\r\n\r\n") {
            Some(pos) => pos,
            None => continue,
        };
        let header_bytes = &chunk[..header_end];
        let mut content = &chunk[header_end + 4..];
        if content.ends_with(b"\r\n") {
            content = &content[..content.len() - 2];
        }

        let headers = String::from_utf8_lossy(header_bytes);
        let mut filename = String::new();

        for line in headers.lines() {
            let lower = line.to_lowercase();
            if lower.starts_with("content-disposition") {
                for kv in line.split(';') {
                    let kv = kv.trim();
                    if let Some(v) = kv.strip_prefix("filename=") {
                        filename = v.trim_matches('"').to_string();
                    }
                }
            }
        }

        if !filename.is_empty() {
            result.push(UploadedFile {
                filename,
                data: content.to_vec(),
            });
        }
    }

    result
}

/// Reduces a (possibly attacker-controlled) filename to a safe basename with
/// no directory components, preventing path traversal on upload.
pub fn sanitize_filename(name: &str) -> String {
    let normalized = name.replace('\\', "/");
    let base = normalized.rsplit('/').next().unwrap_or("");
    let base = base.trim();
    if base.is_empty() || base == "." || base == ".." {
        "upload.bin".to_string()
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_file_part() {
        let body = b"------b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n------b--\r\n";
        let files = parse_multipart(body, "----b");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "a.txt");
        assert_eq!(files[0].data, b"hello");
    }

    #[test]
    fn sanitizes_traversal() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("..\\..\\evil.exe"), "evil.exe");
        assert_eq!(sanitize_filename(".."), "upload.bin");
    }
}
