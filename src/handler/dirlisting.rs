use std::fs;

/// Builds an HTML directory listing for `fs_path`, with links resolved
/// relative to `url_path` (which should end with `/`).
pub fn render(fs_path: &str, url_path: &str) -> Option<Vec<u8>> {
    let mut entries: Vec<(String, bool)> = Vec::new();
    let read_dir = fs::read_dir(fs_path).ok()?;
    for entry in read_dir.flatten() {
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        entries.push((name, file_type.is_dir()));
    }
    entries.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    let mut rows = String::new();
    if url_path != "/" {
        rows.push_str("<tr><td><a href=\"../\">..</a></td><td></td></tr>\n");
    }
    for (name, is_dir) in entries {
        let display = if is_dir {
            format!("{}/", name)
        } else {
            name.clone()
        };
        let href = if is_dir { format!("{}/", name) } else { name };
        let kind = if is_dir { "Directory" } else { "File" };
        rows.push_str(&format!(
            "<tr><td><a href=\"{href}\">{display}</a></td><td class=\"kind\">{kind}</td></tr>\n",
            href = escape_html(&href),
            display = escape_html(&display),
            kind = kind
        ));
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Index of {path}</title>
<style>
  body {{ font-family: -apple-system, Segoe UI, Helvetica, Arial, sans-serif; background:#0f172a; color:#e2e8f0; margin:2rem; }}
  h1 {{ font-size:1.4rem; border-bottom:1px solid #334155; padding-bottom:.5rem; }}
  table {{ width:100%; border-collapse:collapse; margin-top:1rem; }}
  td {{ padding:.35rem .5rem; border-bottom:1px solid #1e293b; }}
  a {{ color:#38bdf8; text-decoration:none; }}
  a:hover {{ text-decoration:underline; }}
  .kind {{ color:#64748b; text-align:right; width:6rem; }}
</style>
</head>
<body>
  <h1>Index of {path}</h1>
  <table>
{rows}  </table>
</body>
</html>
"#,
        path = escape_html(url_path),
        rows = rows
    );

    Some(html.into_bytes())
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
