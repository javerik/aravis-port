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
    /// Byte offset of the element's `<` in the document.
    pub offset: usize,
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
    parse_located(xml).map_err(|(e, _)| e)
}

/// [`parse`], with the byte offset where an error was found.
pub fn parse_located(xml: &str) -> Result<XmlDom, (GenIcamError, Option<usize>)> {
    // With `trim_text` the position before an event can still be on the whitespace in front of
    // the tag, so element offsets are moved forward to their `<`.
    let tag_start = |pos: u64| {
        let pos = pos as usize;
        xml.get(pos..)
            .and_then(|rest| rest.find('<'))
            .map_or(pos, |i| pos + i)
    };
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
        let before = reader.buffer_position();
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
                    offset: tag_start(before),
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
                    offset: tag_start(before),
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
            Err(e) => {
                return Err((
                    GenIcamError::Xml(e.to_string()),
                    Some(reader.error_position() as usize),
                ))
            }
            _ => {}
        }
        buf.clear();
    }

    if let Some(&open) = stack.last() {
        let el = &elements[open];
        return Err((
            GenIcamError::Xml(format!("element <{}> is not closed", el.tag)),
            Some(el.offset),
        ));
    }
    if elements.is_empty() {
        return Err((GenIcamError::Xml("empty document".to_string()), None));
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
        assert!(parse("<Root><Unclosed></Root>").is_err());
        assert!(parse("not xml at all <<<").is_err());
    }

    #[test]
    fn elements_record_the_offset_of_their_tag() {
        let xml = "<Root>\n  <A/>\n  <B>x</B>\n</Root>";
        let dom = parse(xml).unwrap();
        let a = dom.child(dom.root, "A").unwrap();
        let b = dom.child(dom.root, "B").unwrap();
        assert_eq!(dom.get(dom.root).offset, 0);
        assert_eq!(&xml[dom.get(a).offset..][..4], "<A/>");
        assert_eq!(&xml[dom.get(b).offset..][..3], "<B>");
    }

    #[test]
    fn errors_carry_the_offset_where_they_were_found() {
        let xml = "<Root>\n  <A></B>\n</Root>";
        let (_, at) = parse_located(xml).unwrap_err();
        assert_eq!(at, Some(xml.find("</B>").unwrap()));

        let xml = "<Root>\n  <Open>\n";
        let (e, at) = parse_located(xml).unwrap_err();
        assert!(e.to_string().contains("<Open>"), "{e}");
        assert_eq!(at, Some(xml.find("<Open>").unwrap()));
    }
}
