/// Translation bundle used for localized command responses and default embed text.
#[derive(Clone, Copy, Debug)]
pub(super) struct Translations {
    /// Message sent when a channel is registered for newsletters.
    pub(super) register_success: &'static str,
    /// Message sent when a channel is unregistered from newsletters.
    pub(super) unregister_success: &'static str,
    /// Default embed title.
    pub(super) default_title: &'static str,
    /// Default embed description body.
    pub(super) default_description: &'static str,
    /// Default details field title.
    pub(super) details_title: &'static str,
    /// Default details field body.
    pub(super) details_value: &'static str,
    /// Default field title for end date.
    pub(super) ends_title: &'static str,
    /// Default fallback text for missing ban reason.
    pub(super) no_reason: &'static str,
}

/// Builds a locale matcher expression with language and regional fallbacks.
macro_rules! locale_match {
    ($locale:expr, $($lang:literal => $translation:expr),+ $(,)?) => {{
        match $locale {
            $(
                lang if lang.starts_with($lang) => $translation,
            )+
            _ => english(),
        }
    }};
}

/// Returns a translation set selected from user locale first, then guild locale.
pub(super) fn resolve_translations(
    user_locale: Option<&str>,
    guild_locale: Option<&str>,
) -> Translations {
    let locale = normalize_locale(user_locale).or_else(|| normalize_locale(guild_locale));
    match locale {
        Some(locale) => locale_match!(
            locale,
            "ru" => russian(),
            "uk" => ukrainian(),
            "es" => spanish(),
            "de" => german()
        ),
        None => english(),
    }
}

/// Normalizes a locale value by trimming whitespace and converting to lowercase.
pub(super) fn normalize_locale(locale: Option<&str>) -> Option<String> {
    locale
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

fn english() -> Translations {
    Translations {
        register_success: "✅ This channel is now registered for ban newsletters.",
        unregister_success: "✅ This channel has been removed from ban newsletters.",
        default_title: "🚨 New Ban №{id}",
        default_description: "**Intruder:** `{intruder}`\n**Admin:** `{admin}`\n**Reason:** `{reason_display}`\n",
        details_title: "Details",
        details_value: "**Type:** `{type}`\n**Round:** `{round_id}`\n**Server:** `{server}`",
        ends_title: "Ends",
        no_reason: "No reason provided",
    }
}

fn russian() -> Translations {
    Translations {
        register_success: "✅ Этот канал теперь зарегистрирован для рассылки банов.",
        unregister_success: "✅ Этот канал удалён из рассылки банов.",
        default_title: "🚨 Новый бан №{id}",
        default_description: "**Нарушитель:** `{intruder}`\n**Админ:** `{admin}`\n**Причина:** `{reason_display}`\n",
        details_title: "Детали",
        details_value: "**Тип:** `{type}`\n**Раунд:** `{round_id}`\n**Сервер:** `{server}`",
        ends_title: "Окончание",
        no_reason: "Причина не указана",
    }
}

fn ukrainian() -> Translations {
    Translations {
        register_success: "✅ Цей канал тепер зареєстровано для розсилки банів.",
        unregister_success: "✅ Цей канал видалено з розсилки банів.",
        default_title: "🚨 Новий бан №{id}",
        default_description: "**Порушник:** `{intruder}`\n**Адмін:** `{admin}`\n**Причина:** `{reason_display}`\n",
        details_title: "Деталі",
        details_value: "**Тип:** `{type}`\n**Раунд:** `{round_id}`\n**Сервер:** `{server}`",
        ends_title: "Завершення",
        no_reason: "Причину не вказано",
    }
}

fn spanish() -> Translations {
    Translations {
        register_success: "✅ Este canal ahora está registrado para los boletines de baneos.",
        unregister_success: "✅ Este canal fue eliminado de los boletines de baneos.",
        default_title: "🚨 Nuevo baneo №{id}",
        default_description: "**Infractor:** `{intruder}`\n**Admin:** `{admin}`\n**Motivo:** `{reason_display}`\n",
        details_title: "Detalles",
        details_value: "**Tipo:** `{type}`\n**Ronda:** `{round_id}`\n**Servidor:** `{server}`",
        ends_title: "Finaliza",
        no_reason: "No se proporcionó motivo",
    }
}

fn german() -> Translations {
    Translations {
        register_success: "✅ Dieser Kanal ist jetzt für Bann-Newsletters registriert.",
        unregister_success: "✅ Dieser Kanal wurde aus den Bann-Newsletters entfernt.",
        default_title: "🚨 Neuer Bann №{id}",
        default_description: "**Spieler:** `{intruder}`\n**Admin:** `{admin}`\n**Grund:** `{reason_display}`\n",
        details_title: "Details",
        details_value: "**Typ:** `{type}`\n**Runde:** `{round_id}`\n**Server:** `{server}`",
        ends_title: "Endet",
        no_reason: "Kein Grund angegeben",
    }
}
