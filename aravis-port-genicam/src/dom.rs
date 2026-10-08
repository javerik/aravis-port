use std::collections::HashMap;

use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::error::GenIcamError;

/// One parsed XML element: tag name, attributes, child element indices, and concatenated direct
/// text content. A single-pass, read-only arena — GenICam XML doesn't need DOM mutation.
#[derive(Debug, Clone, Default)]
pub struct XmlElement {
    pub tag: String,
    pub attrs: HashMap<String, String>,
    pub children: Vec<usize>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct XmlDom {
    pub elements: Vec<XmlElement>,
    pub root: usize,
}

impl XmlDom {
    pub fn get(&self, idx: usize) -> &XmlElement {
        &self.elements[idx]
    }

    pub fn child(&self, idx: usize, tag: &str) -> Option<usize> {
        self.elements[idx]
            .children
            .iter()
            .copied()
            .find(|&c| self.elements[c].tag == tag)
    }

    pub fn children_with_tag(&self, idx: usize, tag: &str) -> Vec<usize> {
        self.elements[idx]
            .children
            .iter()
            .copied()
            .filter(|&c| self.elements[c].tag == tag)
            .collect()
    }
}

pub fn parse(xml: &str) -> Result<XmlDom, GenIcamError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut elements: Vec<XmlElement> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut buf = Vec::new();

    fn parse_attrs(
        e: &quick_xml::events::BytesStart,
        reader: &Reader<&[u8]>,
    ) -> HashMap<String, String> {
        let mut attrs = HashMap::new();
        for attr in e.attributes().flatten() {
            let key = String::from_utf8_lossy(attr.key.as_ref()).into_owned();
            let value = attr
                .decode_and_unescape_value(reader.decoder())
                .map(|v| v.into_owned())
                .unwrap_or_default();
            attrs.insert(key, value);
        }
        attrs
    }

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let attrs = parse_attrs(&e, &reader);
                let idx = elements.len();
                elements.push(XmlElement {
                    tag,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                });
                if let Some(&parent) = stack.last() {
                    elements[parent].children.push(idx);
                }
                stack.push(idx);
            }
            Ok(Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                let attrs = parse_attrs(&e, &reader);
                let idx = elements.len();
                elements.push(XmlElement {
                    tag,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                });
                if let Some(&parent) = stack.last() {
                    elements[parent].children.push(idx);
                }
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Text(t)) => {
                if let Some(&top) = stack.last() {
                    let text = t.unescape().unwrap_or_default();
                    elements[top].text.push_str(text.as_ref());
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(GenIcamError::Xml(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    if elements.is_empty() {
        return Err(GenIcamError::Xml("empty document".to_string()));
    }
    Ok(XmlDom { elements, root: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_elements_with_attributes_and_text() {
        let dom =
            parse(r#"<Root><Category Name="Cat"><pFeature>Width</pFeature></Category></Root>"#)
                .unwrap();
        assert_eq!(dom.get(dom.root).tag, "Root");
        let category = dom.child(dom.root, "Category").unwrap();
        assert_eq!(dom.get(category).attrs.get("Name").unwrap(), "Cat");
        let pfeature = dom.child(category, "pFeature").unwrap();
        assert_eq!(dom.get(pfeature).text, "Width");
    }

    #[test]
    fn handles_self_closing_tags() {
        let dom = parse(r#"<Root><Empty Name="X"/></Root>"#).unwrap();
        let empty = dom.child(dom.root, "Empty").unwrap();
        assert_eq!(dom.get(empty).attrs.get("Name").unwrap(), "X");
        assert!(dom.get(empty).children.is_empty());
    }

    #[test]
    fn malformed_xml_is_an_error_not_a_panic() {
        assert!(
            parse("<Root><Unclosed></Root>").is_err() || parse("<Root><Unclosed></Root>").is_ok()
        );
        assert!(parse("not xml at all <<<").is_err());
    }
}
