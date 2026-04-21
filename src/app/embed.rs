use log::{debug, trace};
use quick_xml::{Reader, events::Event};

#[derive(Debug, thiserror::Error)]
pub(super) enum EmbedTemplateError {
    #[error("invalid XML: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("invalid embed color `{value}`")]
    InvalidColor { value: String },
}

/// Runtime configuration for building newsletter messages from an XML template.
#[derive(Clone, Debug)]
pub(super) struct EmbedTemplate {
    /// Message title shown above the ban details.
    pub(super) title: String,
    /// Main message body shown in the embed description.
    pub(super) description: String,
    /// Embed color as a 24-bit RGB integer.
    pub(super) color: u32,
}

impl EmbedTemplate {
    /// Creates a built-in fallback template used when no XML file is provided.
    pub(super) fn default_template() -> Self {
        Self {
            title: "🚨 New Ban №{id}".to_owned(),
            description: "**Intruder:** `{intruder}`\n**Admin:** `{admin}`\n**Reason:** {reason_display}\n\n**Details**\n**Type:** `{type}`\n**Round:** `{round_id}`\n**Server:** `{server}`\n\n**Ends:** `{duration_end}`".to_owned(),
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
    /// - zero or more `<line>` elements
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
    /// </embed>
    /// ```
    pub(super) fn from_xml(xml: &str) -> Result<Self, EmbedTemplateError> {
        trace!("parsing XML embed template");
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut title: Option<String> = None;
        let mut description: Option<String> = None;
        let mut color: Option<String> = None;
        let mut lines: Vec<String> = Vec::new();
        let mut current_tag: Option<Vec<u8>> = None;

        loop {
            match reader.read_event()? {
                Event::Start(start) => {
                    current_tag = Some(start.name().as_ref().to_vec());
                }
                Event::Text(text) => {
                    if let Some(tag) = &current_tag {
                        let value = String::from_utf8_lossy(text.as_ref()).into_owned();
                        if !value.is_empty() {
                            match tag.as_slice() {
                                b"title" => title = Some(value),
                                b"description" => description = Some(value),
                                b"color" => color = Some(value),
                                b"line" => lines.push(value),
                                _ => {}
                            }
                        }
                    }
                }
                Event::End(_) => {
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
        } else if !lines.is_empty() {
            template.description = lines.join("\n");
        }
        if let Some(configured_color) = color {
            template.color = parse_color(&configured_color)?;
        }

        debug!(
            "loaded embed template from XML (description len: {}, color: #{:06X})",
            template.description.len(),
            template.color
        );
        Ok(template)
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
