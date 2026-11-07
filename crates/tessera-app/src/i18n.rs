// SPDX-License-Identifier: GPL-3.0-or-later

//! Locale selection. Texts live in `assets/locales/*.yml`.

use tessera_core::settings::Language;

/// Selects the language of `setting`.
pub fn init(setting: Language) {
    let system = sys_locale::get_locale();
    gpui_kit::component::set_locale(locale_for(setting, system.as_deref()));
}

/// The setting first; on `System`, German when the system language is German, else English.
pub(crate) fn locale_for(setting: Language, system: Option<&str>) -> &'static str {
    match (setting, system) {
        (Language::En, _) => "en",
        (Language::De, _) => "de",
        (Language::System, Some(tag)) if tag.to_ascii_lowercase().starts_with("de") => "de",
        (Language::System, _) => "en",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::Language;

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
        let locale = |tag| super::locale_for(Language::System, tag);
        assert_eq!(locale(Some("de-CH")), "de");
        assert_eq!(locale(Some("en-US")), "en");
        assert_eq!(locale(Some("fr-FR")), "en");
        assert_eq!(locale(None), "en");
    }

    #[test]
    fn the_setting_goes_first() {
        assert_eq!(super::locale_for(Language::En, Some("de-CH")), "en");
        assert_eq!(super::locale_for(Language::De, Some("en-US")), "de");
    }
}
