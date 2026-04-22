use log::{debug, trace};
use quick_xml::{Reader, events::Event};

use crate::app::i18n::Translations;

#[derive(Debug, thiserror::Error)]
pub(crate) enum EmbedTemplateError {
    #[error("invalid XML: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("invalid embed color `{value}`")]
    InvalidColor { value: String },
}

/// Runtime configuration for building newsletter messages from an XML template.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EmbedTemplate {
    /// Message title shown above the ban details.
    pub(super) title: String,
    /// Main message body shown in the embed description.
    pub(super) description: String,
    /// Embed fields rendered below the description.
    pub(super) lines: Vec<EmbedLine>,
    /// Embed color as a 24-bit RGB integer.
    pub(super) color: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EmbedLine {
    pub(super) title: String,
    pub(super) value: String,
}

impl EmbedTemplate {
    /// Creates a built-in fallback template used when no XML file is provided.
    pub(super) fn default_template() -> Self {
        Self::default_template_for(crate::app::i18n::resolve_translations(None, None))
    }

    /// Creates a locale-aware fallback template used when no XML file is provided.
    pub(super) fn default_template_for(translations: Translations) -> Self {
        Self {
            title: translations.default_title,
            description: translations.default_description,
            lines: [
                line(&translations.details_title, &translations.details_value),
                line(&translations.ends_title, "`{duration_end}`"),
            ]
            .to_vec(),
            color: poise::serenity_prelude::Color::DARK_RED.0,
        }
    }

    /// Parses an XML template.
    ///
    /// The expected structure is:
    /// - one `<embed>` root
    /// - one optional `<title>` element
    /// - one optional `<description>` element
    /// - one optional `<color>` element (`#RRGGBB`, `0xRRGGBB`, or decimal)
    /// - zero or more `<line title=\"...\">value</line>` elements
    ///
    /// Placeholders in textual fields support `{id}`, `{intruder}`, `{admin}`, `{type}`,
    /// `{round_id}`, `{server}`, `{duration_end}`, `{reason}`, and `{reason_display}`.
    ///
    /// # Example
    ///
    /// ```xml
    /// <embed>
    ///   <title>🚨 New Ban №{id}</title>
    ///   <color>#992D22</color>
    ///   <description>**Intruder:** `{intruder}`</description>
    ///   <line title="Details">**Type:** `{type}`</line>
    ///   <line title="Ends">`{duration_end}`</line>
    /// </embed>
    /// ```
    pub(super) fn from_xml(xml: &str) -> Result<Self, EmbedTemplateError> {
        trace!("parsing XML embed template");
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut title: Option<String> = None;
        let mut description: Option<String> = None;
        let mut color: Option<String> = None;
        let mut lines: Vec<EmbedLine> = Vec::new();
        let mut current_tag: Option<Vec<u8>> = None;
        let mut current_line: Option<EmbedLine> = None;

        loop {
            match reader.read_event()? {
                Event::Start(start) => {
                    let tag = start.name().as_ref().to_vec();
                    if tag.as_slice() == b"line" {
                        let line_title = start
                            .attributes()
                            .filter_map(Result::ok)
                            .find_map(|attr| {
                                let key = attr.key.as_ref();
                                if key == b"title" || key == b"name" {
                                    Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| "Details".to_owned());
                        current_line = Some(line(&line_title, ""));
                    }
                    current_tag = Some(tag);
                }
                Event::Text(text) => {
                    if let Some(tag) = &current_tag {
                        let value = String::from_utf8_lossy(text.as_ref()).into_owned();
                        if !value.is_empty() {
                            match tag.as_slice() {
                                b"title" => title = Some(value),
                                b"description" => description = Some(value),
                                b"color" => color = Some(value),
                                b"line" => {
                                    if let Some(line) = &mut current_line {
                                        line.value.push_str(&value);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Event::End(end) => {
                    if end.name().as_ref() == b"line"
                        && let Some(line) = current_line.take()
                        && !line.value.is_empty()
                    {
                        lines.push(line);
                    }
                    current_tag = None;
                }
                Event::Eof => break,
                _ => {}
            }
        }

        let mut template = Self::default_template();
        if let Some(configured_title) = title {
            template.title = configured_title;
        }
        if let Some(configured_description) = description {
            template.description = configured_description;
        }
        if !lines.is_empty() {
            template.lines = lines;
        }
        if let Some(configured_color) = color {
            template.color = parse_color(&configured_color)?;
        }

        debug!(
            "loaded embed template from XML (description len: {}, lines: {}, color: #{:06X})",
            template.description.len(),
            template.lines.len(),
            template.color
        );
        Ok(template)
    }
}

fn line(title: &str, value: &str) -> EmbedLine {
    EmbedLine {
        title: title.to_owned(),
        value: value.to_owned(),
    }
}

fn parse_color(value: &str) -> Result<u32, EmbedTemplateError> {
    let trimmed = value.trim();
    let parsed = if let Some(hex) = trimmed.strip_prefix('#') {
        u32::from_str_radix(hex, 16).ok()
    } else if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u32::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse::<u32>().ok()
    };

    match parsed {
        Some(color) if color <= 0xFF_FF_FF => Ok(color),
        _ => Err(EmbedTemplateError::InvalidColor {
            value: value.to_owned(),
        }),
    }
}
