use log::{debug, trace};
use quick_xml::{Reader, events::Event};

/// Runtime configuration for building newsletter messages from an XML template.
#[derive(Clone, Debug)]
pub(super) struct EmbedTemplate {
    /// Message title shown above the ban details.
    pub(super) title: String,
    /// Per-line templates for ban details.
    pub(super) lines: Vec<String>,
}

impl EmbedTemplate {
    /// Creates a built-in fallback template used when no XML file is provided.
    pub(super) fn default_template() -> Self {
        Self {
            title: "🚨 **new ban**".to_owned(),
            lines: vec![
                "- id: `{id}`".to_owned(),
                "- intruder: `{intruder}`".to_owned(),
                "- admin: `{admin}`".to_owned(),
                "- type: `{type}`".to_owned(),
                "- round: `{round_id}`".to_owned(),
                "- server: `{server}`".to_owned(),
                "- ends: `{duration_end}`".to_owned(),
                "- reason: `{reason}`".to_owned(),
            ],
        }
    }

    /// Parses an XML template.
    ///
    /// The expected structure is:
    /// - one `<embed>` root
    /// - one optional `<title>` element
    /// - zero or more `<line>` elements
    ///
    /// Placeholders in `<line>` support `{id}`, `{intruder}`, `{admin}`, `{type}`,
    /// `{round_id}`, `{server}`, `{duration_end}`, and `{reason}`.
    ///
    /// # Example
    ///
    /// ```xml
    /// <embed>
    ///   <title>🚨 **New ban notification**</title>
    ///   <line>- id: `{id}`</line>
    ///   <line>- intruder: `{intruder}`</line>
    ///   <line>- reason: `{reason}`</line>
    /// </embed>
    /// ```
    pub(super) fn from_xml(xml: &str) -> Result<Self, quick_xml::Error> {
        trace!("parsing XML embed template");
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut title: Option<String> = None;
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
        if !lines.is_empty() {
            template.lines = lines;
        }

        debug!(
            "loaded embed template with {} line(s) from XML",
            template.lines.len()
        );
        Ok(template)
    }
}
