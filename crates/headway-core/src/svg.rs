//! The pieces of a Tennessine SVG we can read before the full parser exists.

/// The root `viewBox`, in SVG user units (which are Tennessine grid units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewBox {
    pub min_x: f64,
    pub min_y: f64,
    pub width: f64,
    pub height: f64,
}

/// Reads the root `viewBox`.
///
/// Tennessine writes it comma-separated (`viewBox="-95,-118,168,184"`) while the
/// SVG spec also allows whitespace, so both are accepted. This is a deliberate
/// stopgap: it scans for the first `viewBox` attribute rather than parsing the
/// document, and will be replaced when the real parser lands.
pub fn view_box(svg: &str) -> Option<ViewBox> {
    let rest = svg.split("viewBox").nth(1)?.trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    parse_view_box(rest[1..].split(quote).next()?)
}

/// Parses the *value* of a `viewBox` attribute.
pub fn parse_view_box(value: &str) -> Option<ViewBox> {
    let mut nums = value
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::parse::<f64>);
    let mut next = || nums.next()?.ok();

    let vb = ViewBox {
        min_x: next()?,
        min_y: next()?,
        width: next()?,
        height: next()?,
    };
    (vb.width > 0.0 && vb.height > 0.0 && vb.width.is_finite() && vb.height.is_finite()).then_some(vb)
}

/// The markup *inside* the root `<svg>` element, with Tennessine's comment,
/// DOCTYPE and root tag stripped.
///
/// We re-emit our own root element so the `viewBox` can be driven as a camera.
/// Keeping Tennessine's root would mean fighting it for that one attribute, and
/// a CSS transform is not an option: it rasterises the map once at its layout
/// size and scales the bitmap, which is visibly pixelated.
///
/// Another deliberate stopgap, superseded when the real parser lands: it assumes
/// the root tag holds no `>` inside an attribute value, which is true of every
/// Tennessine export.
pub fn root_inner(svg: &str) -> Option<&str> {
    let start = svg.find("<svg")?;
    let open_end = start + svg[start..].find('>')? + 1;
    let close = svg.rfind("</svg")?;
    (close >= open_end).then(|| &svg[open_end..close])
}

#[cfg(test)]
mod tests {
    use super::*;

    const TENNESSINE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" class="metro-map" viewBox="-95,-118,168,184">"#;

    #[test]
    fn reads_the_comma_separated_form_tennessine_emits() {
        let vb = view_box(TENNESSINE).expect("parses");
        assert_eq!(vb, ViewBox { min_x: -95.0, min_y: -118.0, width: 168.0, height: 184.0 });
    }

    #[test]
    fn accepts_whitespace_and_mixed_separators() {
        for s in [
            r#"<svg viewBox="0 0 10 20">"#,
            r#"<svg viewBox="0, 0, 10, 20">"#,
            r#"<svg viewBox=" 0 ,0  10,20 ">"#,
            r#"<svg viewBox='0 0 10 20'>"#,
        ] {
            let vb = view_box(s).unwrap_or_else(|| panic!("failed on {s}"));
            assert_eq!((vb.width, vb.height), (10.0, 20.0), "{s}");
        }
    }

    #[test]
    fn root_inner_strips_the_prolog_and_the_root_tag() {
        let doc = format!("<!--Created for free with Tennessine's Metro Designer--><!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\">{TENNESSINE}<g><path d=\"M0 0\"/></g></svg>");
        assert_eq!(root_inner(&doc), Some("<g><path d=\"M0 0\"/></g>"));
    }

    #[test]
    fn root_inner_handles_an_empty_root_and_rejects_junk() {
        assert_eq!(root_inner("<svg viewBox=\"0 0 1 1\"></svg>"), Some(""));
        assert_eq!(root_inner("no svg here"), None);
        assert_eq!(root_inner("<svg viewBox=\"0 0 1 1\">"), None, "unclosed");
    }

    #[test]
    fn rejects_what_it_cannot_use() {
        assert_eq!(view_box("<svg></svg>"), None, "absent");
        assert_eq!(view_box(r#"<svg viewBox="0 0 10">"#), None, "too few numbers");
        assert_eq!(view_box(r#"<svg viewBox="0 0 0 20">"#), None, "zero width");
        assert_eq!(view_box(r#"<svg viewBox="0 0 -5 20">"#), None, "negative width");
        assert_eq!(view_box(r#"<svg viewBox="a b c d">"#), None, "not numbers");
    }
}
