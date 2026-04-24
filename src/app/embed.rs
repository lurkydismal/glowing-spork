use log::{debug, trace};
use quick_xml::{Reader, events::Event};

use crate::app::i18n::Translations;

#[derive(Debug, thiserror::Error)]
pub(crate) enum EmbedTemplateError {
    #[error("invalid XML: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("invalid embed color `{value}`")]
    InvalidColor { value: String },

    #[error("`<footer>` may appear at most once")]
    DuplicateFooter,

    #[error("`<footer>` must be the last element in `<embed>`")]
    FooterMustBeLast,
}

/// Runtime configuration for building newsletter messages from an XML template.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EmbedTemplate {
    /// Message title shown above the ban details.
    pub(super) title: String,
    /// Embed fields rendered below the description.
    pub(super) lines: Vec<EmbedLine>,
    /// Embed color as a 24-bit RGB integer.
    pub(super) color: u32,
    /// Optional footer shown at the very bottom of the embed.
    pub(super) footer: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct EmbedLine {
    pub(super) title: Option<String>,
    pub(super) value: String,
    /// Render title and value on one line in the field value body.
    pub(super) inline: bool,
    /// Render this field inline in Discord layout.
    pub(super) field_inline: bool,
    /// Parent group id when this line is inside `<group>`.
    pub(super) group_id: Option<usize>,
    /// Whether parent group direction is `row`.
    pub(super) row_group: bool,
    /// Explicit empty line separator (for `<br/>` between blocks).
    pub(super) spacer: bool,
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
            lines: [
                line(
                    Some("\u{200B}"),
                    &translations.default_description,
                    false,
                    false,
                    None,
                    false,
                    false,
                ),
                line(
                    Some(&translations.details_title),
                    &translations.details_value,
                    false,
                    true,
                    None,
                    false,
                    false,
                ),
                line(
                    Some(&translations.ends_title),
                    "`{duration_end}`",
                    false,
                    true,
                    None,
                    false,
                    false,
                ),
            ]
            .to_vec(),
            color: poise::serenity_prelude::Color::DARK_RED.0,
            footer: None,
        }
    }

    /// Parses an XML template.
    ///
    /// The expected structure is:
    /// - one `<embed>` root
    /// - one optional `<title>` element
    /// - one optional `<color>` element (`#RRGGBB`, `0xRRGGBB`, or decimal)
    /// - zero or more `<line title=\"...\" inline=\"...\">value</line>` elements
    /// - zero or more `<group direction=\"row|column\">...</group>` containers with `<line>` children
    /// - one optional `<footer>` element, only once and as the final embed element
    /// - `<br/>` or `<break/>` in textual fields to insert an empty newline
    ///
    /// Placeholders in textual fields support `{id}`, `{intruder}`, `{admin}`, `{type}`,
    /// `{round_id}`, `{server}`, `{created_at}`, `{duration_end}`, `{date}`, `{time}`,
    /// `{date_time}`, `{time_left}`, `{reason}`, and `{reason_display}`.
    ///
    /// # Example
    ///
    /// ```xml
    /// <embed>
    ///   <title>🚨 New Ban №{id}</title>
    ///   <color>#992D22</color>
    ///   <line inline="true" title="Info">**Intruder:** `{intruder}`<br/>**Admin:** `{admin}`</line>
    ///   <group direction="row">
    ///     <line title="Details">**Type:** `{type}`</line>
    ///     <line title="Ends" inline="true">`{duration_end}`</line>
    ///   </group>
    /// </embed>
    /// ```
    pub(super) fn from_xml(xml: &str) -> Result<Self, EmbedTemplateError> {
        trace!("parsing XML embed template");
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut title: Option<String> = None;
        let mut color: Option<String> = None;
        let mut footer: Option<String> = None;
        let mut lines: Vec<EmbedLine> = Vec::new();
        let mut current_tag: Option<Vec<u8>> = None;
        let mut current_line: Option<EmbedLine> = None;
        let mut group_stack: Vec<(usize, bool)> = Vec::new();
        let mut next_group_id = 1usize;
        let mut footer_closed = false;

        loop {
            match reader.read_event()? {
                Event::Start(start) => {
                    let tag = start.name().as_ref().to_vec();
                    if footer_closed {
                        return Err(EmbedTemplateError::FooterMustBeLast);
                    }
                    if tag.as_slice() == b"group" {
                        let row_group = start
                            .attributes()
                            .filter_map(Result::ok)
                            .find_map(|attr| {
                                (attr.key.as_ref() == b"direction").then(|| {
                                    String::from_utf8_lossy(attr.value.as_ref()).into_owned()
                                })
                            })
                            .map(|direction| !direction.eq_ignore_ascii_case("column"))
                            .unwrap_or(true);
                        group_stack.push((next_group_id, row_group));
                        next_group_id += 1;
                        continue;
                    }
                    if tag.as_slice() == b"br" || tag.as_slice() == b"break" {
                        if current_tag.is_some() {
                            append_break(&current_tag, &mut title, &mut current_line);
                        } else {
                            lines.push(spacer_line());
                        }
                        continue;
                    }
                    if tag.as_slice() == b"line" {
                        let line_title =
                            start.attributes().filter_map(Result::ok).find_map(|attr| {
                                let key = attr.key.as_ref();
                                if key == b"title" || key == b"name" {
                                    Some(String::from_utf8_lossy(attr.value.as_ref()).into_owned())
                                } else {
                                    None
                                }
                            });
                        let inline_value = start
                            .attributes()
                            .filter_map(Result::ok)
                            .find_map(|attr| {
                                (attr.key.as_ref() == b"inline").then(|| {
                                    String::from_utf8_lossy(attr.value.as_ref()).into_owned()
                                })
                            })
                            .as_deref()
                            .is_some_and(parse_bool);
                        let (group_id, row_group) =
                            group_stack.last().copied().unwrap_or((0, false));
                        let group_id = (group_id != 0).then_some(group_id);
                        let field_inline = row_group;
                        current_line = Some(line(
                            line_title.as_deref(),
                            "",
                            inline_value,
                            field_inline,
                            group_id,
                            row_group,
                            false,
                        ));
                    }
                    if tag.as_slice() == b"footer" && footer.is_some() {
                        return Err(EmbedTemplateError::DuplicateFooter);
                    }
                    current_tag = Some(tag);
                }
                Event::Empty(empty) => {
                    let tag = empty.name().as_ref().to_vec();
                    if footer_closed {
                        return Err(EmbedTemplateError::FooterMustBeLast);
                    }
                    if tag.as_slice() == b"br" || tag.as_slice() == b"break" {
                        if current_tag.is_some() {
                            append_break(&current_tag, &mut title, &mut current_line);
                        } else {
                            lines.push(spacer_line());
                        }
                    }
                }
                Event::Text(text) => {
                    if let Some(tag) = &current_tag {
                        let value = String::from_utf8_lossy(text.as_ref()).into_owned();
                        if !value.is_empty() {
                            match tag.as_slice() {
                                b"title" => title = Some(value),
                                b"color" => color = Some(value),
                                b"footer" => footer = Some(value),
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
                    if end.name().as_ref() == b"br" || end.name().as_ref() == b"break" {
                        continue;
                    }
                    if end.name().as_ref() == b"group" {
                        group_stack.pop();
                    }
                    if end.name().as_ref() == b"line"
                        && let Some(line) = current_line.take()
                        && !line.value.is_empty()
                    {
                        lines.push(line);
                    }
                    if end.name().as_ref() == b"footer" {
                        footer_closed = true;
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
        if !lines.is_empty() {
            template.lines = lines;
        }
        if let Some(configured_color) = color {
            template.color = parse_color(&configured_color)?;
        }
        if let Some(configured_footer) = footer
            && !configured_footer.is_empty()
        {
            template.footer = Some(configured_footer);
        }

        debug!(
            "loaded embed template from XML (lines: {}, color: #{:06X})",
            template.lines.len(),
            template.color
        );
        Ok(template)
    }
}

fn line(
    title: Option<&str>,
    value: &str,
    inline: bool,
    field_inline: bool,
    group_id: Option<usize>,
    row_group: bool,
    spacer: bool,
) -> EmbedLine {
    EmbedLine {
        title: title.map(ToOwned::to_owned),
        value: value.to_owned(),
        inline,
        field_inline,
        group_id,
        row_group,
        spacer,
    }
}

fn spacer_line() -> EmbedLine {
    line(None, "", false, false, None, false, true)
}

fn append_break(
    current_tag: &Option<Vec<u8>>,
    title: &mut Option<String>,
    current_line: &mut Option<EmbedLine>,
) {
    if let Some(tag) = current_tag {
        match tag.as_slice() {
            b"title" => title.get_or_insert_with(String::new).push_str("\n\n"),
            b"line" => {
                if let Some(line) = current_line {
                    line.value.push_str("\n\n");
                }
            }
            _ => {}
        }
    }
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
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
