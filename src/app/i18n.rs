use std::collections::HashMap;
use std::sync::OnceLock;

use log::warn;
use serde::Deserialize;

/// Translation bundle used for localized command responses and default embed text.
#[derive(Clone, Debug, Deserialize)]
pub(super) struct Translations {
    /// Message sent when a channel is registered for newsletters.
    pub(super) register_success: String,
    /// Message sent when a channel is unregistered from newsletters.
    pub(super) unregister_success: String,
    /// Message sent when a locale override is set for a channel.
    pub(super) locale_set_success: String,
    /// Message sent when locale override is set on a channel that is not registered.
    pub(super) locale_set_requires_register: String,
    /// Message sent when locale value is invalid.
    pub(super) locale_set_invalid: String,
    /// Default embed title.
    pub(super) default_title: String,
    /// Default embed description body.
    pub(super) default_description: String,
    /// Default details field title.
    pub(super) details_title: String,
    /// Default details field body.
    pub(super) details_value: String,
    /// Default field title for end date.
    pub(super) ends_title: String,
    /// Default fallback text for missing ban reason.
    pub(super) no_reason: String,
}

#[derive(Debug, Deserialize)]
struct I18nFile {
    locales: HashMap<String, Translations>,
}

static TRANSLATIONS: OnceLock<I18nFile> = OnceLock::new();

/// Returns a translation set selected from user locale first, then guild locale.
pub(super) fn resolve_translations(
    user_locale: Option<&str>,
    guild_locale: Option<&str>,
) -> Translations {
    let locale = normalize_locale(user_locale).or_else(|| normalize_locale(guild_locale));
    let store = TRANSLATIONS.get_or_init(load_translations);

    if let Some(locale) = locale
        && let Some(translations) = find_locale_translation(store, &locale)
    {
        return translations.clone();
    }

    if let Some(english) = find_locale_translation(store, "en") {
        return english.clone();
    }

    built_in_english()
}

/// Returns all configured locale keys sorted alphabetically.
pub(super) fn available_locales() -> Vec<String> {
    let store = TRANSLATIONS.get_or_init(load_translations);
    let mut locales: Vec<String> = store.locales.keys().cloned().collect();
    locales.sort_unstable();
    locales
}

/// Reports whether a locale key exists in the configured translation bundle.
pub(super) fn is_supported_locale(locale: &str) -> bool {
    let store = TRANSLATIONS.get_or_init(load_translations);
    store.locales.contains_key(locale)
}

/// Normalizes a locale value by trimming whitespace and converting to lowercase.
pub(super) fn normalize_locale(locale: Option<&str>) -> Option<String> {
    locale
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn find_locale_translation<'a>(store: &'a I18nFile, locale: &str) -> Option<&'a Translations> {
    if let Some(exact) = store.locales.get(locale) {
        return Some(exact);
    }

    store
        .locales
        .iter()
        .find_map(|(key, value)| locale.starts_with(key).then_some(value))
}

fn load_translations() -> I18nFile {
    let path = match std::env::var("I18_FILE") {
        Ok(path) => path,
        Err(error) => {
            warn!("I18_FILE is not set: {error}. Falling back to built-in translations.");
            return built_in_file();
        }
    };

    if !path.ends_with(".jsonc") {
        warn!("I18_FILE should point to a .jsonc file, got `{path}`. Attempting to parse anyway.");
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) => {
            warn!("failed to read I18_FILE `{path}`: {error}. Using built-in translations.");
            return built_in_file();
        }
    };

    match json5::from_str::<I18nFile>(&content) {
        Ok(file) => file,
        Err(error) => {
            warn!(
                "failed to parse I18_FILE `{path}` as jsonc/json5: {error}. Using built-in translations."
            );
            built_in_file()
        }
    }
}

fn built_in_file() -> I18nFile {
    let mut locales = HashMap::new();
    locales.insert("en".to_owned(), built_in_english());
    I18nFile { locales }
}

fn built_in_english() -> Translations {
    Translations {
        register_success: "✅ This channel is now registered for ban newsletters.".to_owned(),
        unregister_success: "✅ This channel has been removed from ban newsletters.".to_owned(),
        locale_set_success: "✅ This channel locale is now set to `{locale}`.".to_owned(),
        locale_set_requires_register:
            "⚠️ This channel is not registered yet. Run `/register` first.".to_owned(),
        locale_set_invalid: "⚠️ Unsupported locale. Pick one from the autocomplete list."
            .to_owned(),
        default_title: "🚨 New Ban №{id}".to_owned(),
        default_description:
            "**Intruder:** `{intruder}`\n**Admin:** `{admin}`\n**Reason:** `{reason_display}`\n"
                .to_owned(),
        details_title: "Details".to_owned(),
        details_value: "**Type:** `{type}`\n**Round:** `{round_id}`\n**Server:** `{server}`"
            .to_owned(),
        ends_title: "Ends".to_owned(),
        no_reason: "No reason provided".to_owned(),
    }
}
