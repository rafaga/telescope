//! Interface language.
//!
//! The texts live in `locales/<code>.toml` (one file per language, same keys in
//! every file) and are embedded at compile time by `rust_i18n::i18n!` in
//! `lib.rs`; the UI reads them with `t!("section.key")`. A key missing from a
//! language falls back to English. Adding a language is adding its file: the
//! selector in Settings -> General lists every file it finds, by the name in
//! its own `language.name` key.
//!
//! Only what the user reads is translated. `tracing` output and the
//! messages in the log panel stay in English, so bug reports read the same
//! whatever the interface language.
//!
//! The chosen language is `language` in `telescope.toml`'s `[ui]` table:
//! `"auto"` (the operating system's language, when Telescope has it; English
//! otherwise) or a file name such as `"es"`.

/// `[ui] language` value that follows the operating system's language.
pub(crate) const AUTO: &str = "auto";

/// Language used when the operating system's one isn't available.
const FALLBACK: &str = "en";

/// The languages there is a `locales/` file for, sorted by code.
pub(crate) fn available() -> Vec<String> {
    let mut locales: Vec<String> = rust_i18n::available_locales!()
        .into_iter()
        .map(|code| code.into_owned())
        .collect();
    locales.sort_unstable();
    locales
}

/// A language's own name (`"Español"`, `"日本語"`), as the selector shows it.
pub(crate) fn display_name(code: &str) -> String {
    t!("language.name", locale = code).into_owned()
}

/// The language `setting` stands for: itself when there is a file for it,
/// the operating system's language for `"auto"`, English otherwise.
pub(crate) fn resolve(setting: &str) -> String {
    let wanted = if setting == AUTO {
        sys_locale::get_locale().unwrap_or_default()
    } else {
        setting.to_owned()
    };
    best_match(&wanted, &available()).unwrap_or_else(|| FALLBACK.to_owned())
}

/// Finds `wanted` (a BCP 47 tag such as `es-MX` or `zh-Hans-CN`) among
/// `available`: the exact tag first, then its primary language (`es`).
fn best_match<S: AsRef<str>>(wanted: &str, available: &[S]) -> Option<String> {
    let wanted = wanted.replace('_', "-");
    let primary = wanted.split('-').next().unwrap_or_default();
    available
        .iter()
        .find(|code| code.as_ref().eq_ignore_ascii_case(&wanted))
        .or_else(|| {
            available
                .iter()
                .find(|code| code.as_ref().eq_ignore_ascii_case(primary))
        })
        .map(|code| code.as_ref().to_owned())
}

/// Switches the interface to `setting` (see [`resolve`]). egui repaints the
/// whole UI every frame, so the change shows up on the next one.
pub(crate) fn apply_language(setting: &str) {
    rust_i18n::set_locale(&resolve(setting));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    /// Every `a.b.c` key of a locale file.
    fn keys(table: &toml::Table, prefix: &str, out: &mut BTreeSet<String>) {
        for (key, value) in table {
            let full = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            match value {
                toml::Value::Table(inner) => keys(inner, &full, out),
                _ => {
                    out.insert(full);
                }
            }
        }
    }

    fn locale_keys(code: &str) -> BTreeSet<String> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("locales")
            .join(format!("{code}.toml"));
        let text = std::fs::read_to_string(&path).unwrap();
        let table: toml::Table = toml::from_str(&text).unwrap();
        let mut out = BTreeSet::new();
        keys(&table, "", &mut out);
        out
    }

    // Every language has exactly the keys English has: a missing one would
    // silently show English, an extra one is a typo or a leftover.
    #[test]
    fn every_locale_has_the_same_keys_as_english() {
        let english = locale_keys("en");
        for code in available() {
            let other = locale_keys(&code);
            let missing: Vec<_> = english.difference(&other).collect();
            let extra: Vec<_> = other.difference(&english).collect();
            assert!(
                missing.is_empty() && extra.is_empty(),
                "locales/{code}.toml: missing {missing:?}, extra {extra:?}"
            );
        }
    }

    // `%{name}` placeholders must match too, or a translation drops a value.
    #[test]
    fn every_locale_keeps_the_placeholders() {
        let placeholders = |text: &str| -> BTreeSet<String> {
            text.split("%{")
                .skip(1)
                .filter_map(|rest| rest.split('}').next())
                .map(str::to_owned)
                .collect()
        };
        for code in available() {
            for key in locale_keys("en") {
                let english = t!(&key, locale = "en");
                let other = t!(&key, locale = &code);
                assert_eq!(
                    placeholders(&english),
                    placeholders(&other),
                    "locales/{code}.toml: {key}"
                );
            }
        }
    }

    #[test]
    fn english_and_spanish_are_available() {
        let locales = available();
        assert!(locales.iter().any(|code| code == "en"));
        assert!(locales.iter().any(|code| code == "es"));
    }

    #[test]
    fn a_region_falls_back_to_its_language() {
        let available = ["en", "es"];
        assert_eq!(best_match("es-MX", &available).as_deref(), Some("es"));
        assert_eq!(best_match("es_ES", &available).as_deref(), Some("es"));
        assert_eq!(best_match("EN", &available).as_deref(), Some("en"));
        assert_eq!(best_match("ja-JP", &available), None);
        assert_eq!(best_match("", &available), None);
    }

    #[test]
    fn an_explicit_language_wins_and_an_unknown_one_is_english() {
        assert_eq!(resolve("es"), "es");
        assert_eq!(resolve("xx"), "en");
    }

    #[test]
    fn each_language_names_itself() {
        assert_eq!(display_name("en"), "English");
        assert_eq!(display_name("es"), "Español");
    }
}
