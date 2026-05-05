use std::sync::{OnceLock, RwLock};

/// Translation bundle used for localized command responses and default embed text.
#[derive(Clone, Debug)]
pub(super) struct Translations {
    pub(super) register_success: String,
    pub(super) unregister_success: String,
    pub(super) locale_set_success: String,
    pub(super) locale_set_requires_register: String,
    pub(super) locale_set_invalid: String,
    pub(super) default_title: String,
    pub(super) default_description: String,
    pub(super) details_title: String,
    pub(super) details_value: String,
    pub(super) ends_title: String,
    pub(super) no_reason: String,
}

static EMBED_LOCALES: OnceLock<RwLock<Vec<String>>> = OnceLock::new();

pub(super) fn set_available_locales(locales: Vec<String>) {
    let store = EMBED_LOCALES.get_or_init(|| RwLock::new(vec!["en".to_owned()]));
    let mut guard = store.write().expect("locale lock poisoned");
    *guard = if locales.is_empty() {
        vec!["en".to_owned()]
    } else {
        locales
    };
}

pub(super) fn resolve_translations(
    _user_locale: Option<&str>,
    _guild_locale: Option<&str>,
) -> Translations {
    built_in_english()
}

pub(super) fn available_locales() -> Vec<String> {
    EMBED_LOCALES
        .get_or_init(|| RwLock::new(vec!["en".to_owned()]))
        .read()
        .expect("locale lock poisoned")
        .clone()
}

pub(super) fn is_supported_locale(locale: &str) -> bool {
    available_locales()
        .iter()
        .any(|configured| configured == locale)
}

pub(super) fn normalize_locale(locale: Option<&str>) -> Option<String> {
    locale
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
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
        default_description: "**Intruder:** `{intruder}`
**Admin:** `{admin}`
**Reason:** `{reason_display}`
"
        .to_owned(),
        details_title: "Details".to_owned(),
        details_value: "**Type:** `{type}`
**Round:** `{round_id}`
**Server:** `{server}`"
            .to_owned(),
        ends_title: "Ends".to_owned(),
        no_reason: "No reason provided".to_owned(),
    }
}
