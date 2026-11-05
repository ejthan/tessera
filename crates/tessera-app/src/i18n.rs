// SPDX-License-Identifier: GPL-3.0-or-later

//! Locale selection. Texts live in `assets/locales/*.yml`.

/// Selects German when the system language is German, English otherwise.
pub fn init() {
    gpui_kit::component::set_locale(locale_for(sys_locale::get_locale().as_deref()));
}

pub(crate) fn locale_for(system: Option<&str>) -> &'static str {
    match system {
        Some(tag) if tag.to_ascii_lowercase().starts_with("de") => "de",
        _ => "en",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    fn keys(yaml: &str) -> BTreeSet<&str> {
        yaml.lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with('_') && l.contains(':'))
            .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim()))
            .collect()
    }

    #[test]
    fn en_and_de_have_same_keys() {
        let en = keys(include_str!("../../../assets/locales/en.yml"));
        let de = keys(include_str!("../../../assets/locales/de.yml"));
        assert!(!en.is_empty());
        assert_eq!(en, de);
    }

    #[test]
    fn german_lookup_works() {
        assert_eq!(
            rust_i18n::t!("status.not_connected", locale = "de"),
            "Nicht verbunden"
        );
    }

    #[test]
    fn picks_german_for_german_system_locale() {
        assert_eq!(super::locale_for(Some("de-CH")), "de");
        assert_eq!(super::locale_for(Some("en-US")), "en");
        assert_eq!(super::locale_for(Some("fr-FR")), "en");
        assert_eq!(super::locale_for(None), "en");
    }
}
