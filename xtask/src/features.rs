//! The feature matrix gate: every crate that declares features is checked without default
//! features, with them and with every stable feature.

/// The `cargo check` arguments (after `check`) that cover one crate; empty for a crate without
/// features. Features named `unstable` or prefixed `unstable-` are never enabled together.
pub fn invocations(krate: &str, features: &[String]) -> Vec<Vec<String>> {
    if features.is_empty() {
        return Vec::new();
    }
    let base = |extra: &[&str]| -> Vec<String> {
        ["-p", krate]
            .into_iter()
            .chain(extra.iter().copied())
            .map(str::to_owned)
            .collect()
    };
    let mut stable: Vec<&str> = features
        .iter()
        .map(String::as_str)
        .filter(|f| *f != "default" && *f != "unstable" && !f.starts_with("unstable-"))
        .collect();
    stable.sort_unstable();
    let mut out = vec![base(&["--no-default-features"]), base(&[])];
    if !stable.is_empty() {
        let list = stable.join(",");
        out.push(base(&["--features", &list]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn crate_without_features_is_skipped() {
        assert!(invocations("a", &[]).is_empty());
    }

    #[test]
    fn crate_with_features_gets_three_checks_without_unstable_ones() {
        let found = invocations(
            "a",
            &names(&["serde", "default", "unstable", "unstable-x", "io"]),
        );
        assert_eq!(
            found,
            vec![
                names(&["-p", "a", "--no-default-features"]),
                names(&["-p", "a"]),
                names(&["-p", "a", "--features", "io,serde"]),
            ]
        );
    }

    #[test]
    fn crate_with_only_default_or_unstable_features_skips_the_all_features_check() {
        assert_eq!(invocations("a", &names(&["default", "unstable"])).len(), 2);
    }
}
