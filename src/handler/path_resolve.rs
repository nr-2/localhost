use std::path::{Component, Path, PathBuf};

use crate::config::Route;

/// Resolves a URL path against a route's `root`, producing a filesystem
/// path. Returns `None` if the route has no root configured, or if the
/// resulting path would escape `root` via `..` components (path traversal).
pub fn resolve(route: &Route, req_path: &str) -> Option<PathBuf> {
    let root = route.root.as_ref()?;
    let rel = req_path
        .strip_prefix(route.path.as_str())
        .unwrap_or(req_path);
    let rel = rel.trim_start_matches('/');

    let mut result = PathBuf::from(root);
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => result.push(part),
            Component::ParentDir => return None,
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Route;

    fn route(path: &str, root: &str) -> Route {
        let mut r = Route::new(path.to_string());
        r.root = Some(root.to_string());
        r
    }

    #[test]
    fn resolves_simple_path() {
        let r = route("/", "./www/html");
        assert_eq!(
            resolve(&r, "/index.html"),
            Some(PathBuf::from("./www/html/index.html"))
        );
    }

    #[test]
    fn resolves_prefixed_route() {
        let r = route("/uploads", "./www/uploads");
        assert_eq!(
            resolve(&r, "/uploads/a.txt"),
            Some(PathBuf::from("./www/uploads/a.txt"))
        );
    }

    #[test]
    fn rejects_path_traversal() {
        let r = route("/", "./www/html");
        assert_eq!(resolve(&r, "/../../etc/passwd"), None);
        assert_eq!(resolve(&r, "/foo/../../etc/passwd"), None);
    }
}
