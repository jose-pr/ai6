//! The SVG a view may hold: an allow-list of drawing elements and attributes, checked here so
//! the app can draw what it is sent without checking it again.
//!
//! Anything outside the list is refused with the reason, never removed: she fixes the drawing
//! and shows it again. Nothing in an accepted SVG runs, links or loads: there is no script,
//! style, event attribute, `href`, image, foreign content, `use`, animation or filter, and a
//! `url(...)` reference names an `id` of the same drawing. The accepted tree is written out
//! again, and that canonical text is what a view holds.

use std::collections::HashSet;
use std::fmt::Write as _;

use roxmltree::{Document, Node, NodeType, ParsingOptions};

use super::views::ViewLimits;

/// The SVG namespace; an element in any other is refused.
const SVG_NS: &str = "http://www.w3.org/2000/svg";

const ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "title",
    "desc",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "linearGradient",
    "radialGradient",
    "stop",
    "clipPath",
];

/// Elements whose text is drawn or read: text anywhere else is refused.
const TEXT_ELEMENTS: &[&str] = &["title", "desc", "text", "tspan"];

/// Geometry and presentation attributes, and `id`.
const ATTRIBUTES: &[&str] = &[
    "id",
    // Geometry.
    "viewBox",
    "preserveAspectRatio",
    "version",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "fx",
    "fy",
    "fr",
    "width",
    "height",
    "d",
    "points",
    "transform",
    "dx",
    "dy",
    "rotate",
    "textLength",
    "lengthAdjust",
    "pathLength",
    "offset",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "clipPathUnits",
    // Presentation.
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "opacity",
    "stop-color",
    "stop-opacity",
    "clip-path",
    "clip-rule",
    "color",
    "display",
    "visibility",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "alignment-baseline",
    "baseline-shift",
    "letter-spacing",
    "word-spacing",
    "text-decoration",
    "vector-effect",
    "paint-order",
    "shape-rendering",
    "text-rendering",
];

/// Checks `text` against the allow-list and returns its canonical form, or why it is refused.
///
/// The reason names the first element, attribute or limit broken. Accepted text needs a root
/// `<svg>` with a `viewBox` and a `<title>`; the canonical form declares the SVG namespace on
/// the root, keeps attributes in their order, drops comments and the whitespace between
/// elements, and parses again to itself.
pub(crate) fn sanitize(text: &str, limits: &ViewLimits) -> Result<String, String> {
    if text.len() > limits.svg_bytes {
        return Err(format!("an SVG is at most {} bytes", limits.svg_bytes));
    }
    // roxmltree refuses a DTD with entities but lets an empty one through; neither belongs.
    let lower = text.to_ascii_lowercase();
    if lower.contains("<!doctype") || lower.contains("<!entity") {
        return Err("an SVG has no DOCTYPE or entity declarations".to_owned());
    }
    let options = ParsingOptions {
        allow_dtd: false,
        // Elements, text and comments together; a bound on memory before the element count.
        nodes_limit: u32::try_from(limits.svg_elements.saturating_mul(4)).unwrap_or(u32::MAX),
        entity_resolver: None,
    };
    let document = Document::parse_with_options(text, options).map_err(|error| match error {
        roxmltree::Error::DtdDetected => "an SVG has no DOCTYPE or entity declarations".to_owned(),
        roxmltree::Error::NodesLimitReached => {
            format!("an SVG has at most {} elements", limits.svg_elements)
        }
        error => format!("the SVG is not well-formed XML: {error}"),
    })?;
    let root = document.root_element();
    if root.tag_name().name() != "svg" {
        return Err("the root element of an SVG is <svg>".to_owned());
    }
    if !root.has_attribute("viewBox") {
        return Err("the <svg> root has a viewBox".to_owned());
    }
    view_box(root.attribute("viewBox").unwrap_or_default())?;
    let titled = root.children().any(|child| {
        child.is_element()
            && child.tag_name().name() == "title"
            && child.text().is_some_and(|title| !title.trim().is_empty())
    });
    if !titled {
        return Err("the <svg> root has a <title> saying what it shows".to_owned());
    }

    let mut ids = HashSet::new();
    let mut elements = 0;
    check(root, 1, limits, &mut ids, &mut elements)?;
    references(root, &ids)?;

    let mut canonical = String::with_capacity(text.len());
    write(root, true, &mut canonical);
    Ok(canonical)
}

/// A `viewBox` of four finite numbers with a positive width and height.
fn view_box(value: &str) -> Result<(), String> {
    let numbers: Vec<f64> = value
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<f64>().ok().filter(|number| number.is_finite()))
        .collect::<Option<_>>()
        .unwrap_or_default();
    match numbers.as_slice() {
        [_, _, width, height] if *width > 0.0 && *height > 0.0 => Ok(()),
        _ => Err(
            "a viewBox is four numbers: min-x min-y width height, both sizes above 0".to_owned(),
        ),
    }
}

/// Text a person reads without characters that render as nothing or control characters.
fn visible(what: &str, text: &str) -> Result<(), String> {
    if let Some(character) = text.chars().find(|character| {
        ai6::directorates::m::vetting::is_invisible(*character)
            || (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    }) {
        return Err(format!(
            "{what} holds a hidden or control character (U+{:04X}); write it without one",
            u32::from(character)
        ));
    }
    Ok(())
}

fn check(
    node: Node<'_, '_>,
    depth: usize,
    limits: &ViewLimits,
    ids: &mut HashSet<String>,
    elements: &mut usize,
) -> Result<(), String> {
    let name = node.tag_name().name();
    if node
        .tag_name()
        .namespace()
        .is_some_and(|namespace| namespace != SVG_NS)
    {
        return Err(format!("the element <{name}> is not in the SVG namespace"));
    }
    if name == "svg" && depth > 1 {
        return Err("<svg> is only the root element".to_owned());
    }
    if !ELEMENTS.contains(&name) {
        return Err(refused_element(name));
    }
    *elements += 1;
    if *elements > limits.svg_elements {
        return Err(format!(
            "an SVG has at most {} elements",
            limits.svg_elements
        ));
    }
    if depth > limits.svg_depth {
        return Err(format!(
            "an SVG nests elements at most {} deep",
            limits.svg_depth
        ));
    }
    for attribute in node.attributes() {
        let key = attribute.name();
        if attribute.namespace().is_some() || !ATTRIBUTES.contains(&key) {
            let shown = match attribute.namespace() {
                Some(_) if key == "href" => "xlink:href".to_owned(),
                Some(_) => format!("{{{}}}{key}", attribute.namespace().unwrap_or_default()),
                None => key.to_owned(),
            };
            return Err(refused_attribute(&shown));
        }
        visible(&format!("the attribute {key}"), attribute.value())?;
        if key == "id" {
            let id = attribute.value();
            if id.is_empty()
                || id.len() > limits.svg_id_bytes
                || !id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            {
                return Err(format!(
                    "an id is 1 to {} letters, digits, '-', '_' or '.', not {id:?}",
                    limits.svg_id_bytes
                ));
            }
            if !ids.insert(id.to_owned()) {
                return Err(format!("the id {id:?} is used twice"));
            }
        }
    }
    for child in node.children() {
        match child.node_type() {
            NodeType::Element => check(child, depth + 1, limits, ids, elements)?,
            NodeType::Text => {
                let text = child.text().unwrap_or_default();
                if !TEXT_ELEMENTS.contains(&name) && !text.trim().is_empty() {
                    return Err(format!(
                        "text belongs in <text>, <tspan>, <title> or <desc>, not <{name}>"
                    ));
                }
                visible("text", text)?;
            }
            NodeType::PI => {
                return Err("an SVG has no processing instructions (<?...?>)".to_owned());
            }
            NodeType::Comment | NodeType::Root => {}
        }
    }
    Ok(())
}

fn refused_element(name: &str) -> String {
    let why = match name {
        "script" => "nothing in an SVG runs",
        "style" => "presentation is set with attributes, not style sheets",
        "foreignObject" => "an SVG holds no foreign content",
        "image" | "feImage" => "an SVG loads no images",
        "use" => "an SVG copies no content by reference",
        "a" => "an SVG holds no links",
        "filter" => "filters are not drawn",
        name if name.starts_with("animate") || name == "set" => "nothing in an SVG moves",
        _ => "it is not on the list of drawing elements",
    };
    format!("the SVG element <{name}> is refused: {why}")
}

fn refused_attribute(name: &str) -> String {
    let why = if name.starts_with("on") {
        "nothing in an SVG runs"
    } else if name.ends_with("href") {
        "an SVG links to and loads nothing; refer to a local id with url(#id)"
    } else if name == "style" || name == "class" {
        "presentation is set with attributes such as fill and stroke"
    } else {
        "it is not a geometry or presentation attribute"
    };
    format!("the SVG attribute {name} is refused: {why}")
}

/// Every `url(...)` in an attribute names an `id` of this drawing: `url(#id)`.
fn references(node: Node<'_, '_>, ids: &HashSet<String>) -> Result<(), String> {
    for attribute in node.attributes() {
        let mut rest = attribute.value();
        while let Some(at) = rest.to_ascii_lowercase().find("url(") {
            let after = &rest[at + 4..];
            let close = after.find(')').ok_or("a url( reference is not closed")?;
            let target = after[..close].trim();
            let local = target.strip_prefix('#').filter(|id| ids.contains(*id));
            if local.is_none() {
                return Err(format!(
                    "{} refers to {target:?}; only url(#id) to an id of this SVG is allowed",
                    attribute.name()
                ));
            }
            rest = &after[close + 1..];
        }
    }
    node.children()
        .filter(Node::is_element)
        .try_for_each(|child| references(child, ids))
}

fn escape(text: &str, attribute: bool, out: &mut String) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            '\t' if attribute => out.push_str("&#9;"),
            character => out.push(character),
        }
    }
}

fn write(node: Node<'_, '_>, root: bool, out: &mut String) {
    let name = node.tag_name().name();
    out.push('<');
    out.push_str(name);
    if root {
        let _ = write!(out, " xmlns=\"{SVG_NS}\"");
    }
    for attribute in node.attributes() {
        out.push(' ');
        out.push_str(attribute.name());
        out.push_str("=\"");
        escape(attribute.value(), true, out);
        out.push('"');
    }
    let keeps_text = TEXT_ELEMENTS.contains(&name);
    let children: Vec<Node<'_, '_>> = node
        .children()
        .filter(|child| match child.node_type() {
            NodeType::Element => true,
            NodeType::Text => keeps_text,
            _ => false,
        })
        .collect();
    if children.is_empty() {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in children {
        if child.is_element() {
            write(child, false, out);
        } else {
            escape(child.text().unwrap_or_default(), false, out);
        }
    }
    out.push_str("</");
    out.push_str(name);
    out.push('>');
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_SVG_BYTES: usize = 32 * 1024;

    fn sanitize(text: &str) -> Result<String, String> {
        super::sanitize(text, &ViewLimits::default())
    }

    const DRAWING: &str = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 50">
  <title>Two Stations &amp; a link</title>
  <!-- a comment is not content -->
  <defs>
    <linearGradient id="glow"><stop offset="0" stop-color="#0af"/></linearGradient>
  </defs>
  <rect x="0" y="0" width="40" height="20" fill="url(#glow)"/>
  <text x="5" y="40">hq &lt;-&gt; worker</text>
</svg>"##;

    #[test]
    fn an_id_is_held_to_its_setting() {
        let drawing = |id: &str| {
            format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\">                 <title>t</title><rect id=\"{id}\"/></svg>"
            )
        };
        let tight = ViewLimits {
            svg_id_bytes: 4,
            ..ViewLimits::default()
        };
        assert!(super::sanitize(&drawing("abcd"), &tight).is_ok());
        let refused = super::sanitize(&drawing("abcde"), &tight).unwrap_err();
        assert!(refused.contains("1 to 4"), "{refused}");
        assert!(sanitize(&drawing(&"a".repeat(64))).is_ok());
        assert!(sanitize(&drawing(&"a".repeat(65))).is_err());
    }

    #[test]
    fn an_allowed_drawing_is_kept_in_canonical_form_which_parses_to_itself() {
        let canonical = sanitize(DRAWING).unwrap();
        assert!(canonical.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox="));
        assert!(!canonical.contains("comment"));
        assert!(canonical.contains("<title>Two Stations &amp; a link</title>"));
        assert!(canonical.contains("hq &lt;-&gt; worker"));
        assert_eq!(sanitize(&canonical).unwrap(), canonical);
    }

    #[test]
    fn what_could_run_link_or_load_is_refused_with_a_reason() {
        let wrap = |inner: &str| {
            format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" \
                 viewBox=\"0 0 10 10\"><title>t</title>{inner}</svg>"
            )
        };
        for (inner, reason) in [
            ("<script>alert(1)</script>", "<script>"),
            ("<rect onclick=\"x\" width=\"1\" height=\"1\"/>", "onclick"),
            ("<a href=\"https://example.com\"><rect/></a>", "<a>"),
            ("<rect xlink:href=\"#t\"/>", "xlink:href"),
            ("<style>rect{fill:red}</style>", "<style>"),
            ("<rect class=\"x\"/>", "class"),
            ("<filter id=\"f\"/>", "<filter>"),
            ("<animate attributeName=\"x\"/>", "<animate>"),
            ("<set attributeName=\"x\"/>", "<set>"),
            ("<rect fill=\"url( https://example.com/x )\"/>", "url(#id)"),
            ("<rect fill=\"url(#nowhere)\"/>", "url(#id)"),
            ("<g>loose text</g>", "text belongs"),
            ("<svg viewBox=\"0 0 1 1\"/>", "<svg>"),
            ("<?php echo 1 ?>", "processing instructions"),
            ("<text>a\u{200B}b</text>", "hidden"),
            ("<rect id=\"a\"/><rect id=\"a\"/>", "used twice"),
        ] {
            let refused = sanitize(&wrap(inner)).unwrap_err();
            assert!(refused.contains(reason), "{inner}: {refused}");
        }
    }

    #[test]
    fn the_root_needs_a_view_box_and_a_title_within_the_limits() {
        let svg = |attributes: &str, inner: &str| {
            format!("<svg xmlns=\"http://www.w3.org/2000/svg\" {attributes}>{inner}</svg>")
        };
        assert!(
            sanitize(&svg("", "<title>t</title>"))
                .unwrap_err()
                .contains("viewBox")
        );
        assert!(
            sanitize(&svg("viewBox=\"0 0 0 1\"", "<title>t</title>"))
                .unwrap_err()
                .contains("viewBox")
        );
        assert!(
            sanitize(&svg("viewBox=\"0 0 1 1\"", "<title> </title>"))
                .unwrap_err()
                .contains("<title>")
        );
        let deep = format!("{}{}", "<g>".repeat(16), "</g>".repeat(16));
        assert!(
            sanitize(&svg(
                "viewBox=\"0 0 1 1\"",
                &format!("<title>t</title>{deep}")
            ))
            .unwrap_err()
            .contains("deep")
        );
        let big = "x".repeat(MAX_SVG_BYTES);
        assert!(
            sanitize(&svg(
                "viewBox=\"0 0 1 1\"",
                &format!("<title>{big}</title>")
            ))
            .unwrap_err()
            .contains("bytes")
        );
        assert!(sanitize("<html/>").unwrap_err().contains("root element"));
        assert!(
            sanitize("<!DOCTYPE svg><svg viewBox=\"0 0 1 1\"><title>t</title></svg>")
                .unwrap_err()
                .contains("DOCTYPE")
        );
    }
}
