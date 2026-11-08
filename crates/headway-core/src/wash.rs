//! Fading the network out so a planned journey stands clear of it.

use crate::{LineId, Network};
use std::collections::BTreeSet;

/// The grey a faded line is mixed towards. Picked for Tennessine's near-white
/// paper: dark enough that a faded line still reads as a line over the grid,
/// light enough that it stops competing with the journey.
pub const WASH: &str = "#bcbcc6";

/// How much of a line's own colour survives the fade, as a CSS percentage.
/// Enough to tell a faded red from a faded green at a glance, not enough to
/// mistake either for a route.
pub const KEPT_PERCENT: u8 = 20;

/// A stylesheet that fades every line except `keep` to a pale tint of its own
/// colour, leaving the journey the only saturated thing on the map.
///
/// Lines are addressed by their stroke exactly as the file spells it, because
/// in Tennessine's markup that is the whole of a line's identity — there is no
/// class or id to hang a rule on, and tagging the paths would mean rewriting
/// half a megabyte of SVG on every query. Two consequences fall out of that:
///
/// * Lines drawn in one colour fade or stay together. These are the same lines
///   the parser already cannot tell apart, so the map was never going to
///   distinguish them.
/// * The key's swatches fade with the lines they stand for, which turns the
///   legend into a list of what the journey uses.
///
/// Station ticks are drawn in their line's colour and so fade with it; an
/// interchange's circle is black on white and never fades.
pub fn wash_except(net: &Network, keep: &[LineId]) -> String {
    // A colour shared with a kept line cannot be faded without fading the
    // journey, so the journey wins.
    let kept: BTreeSet<&str> = keep
        .iter()
        .flat_map(|id| net.line(*id).strokes.iter())
        .map(String::as_str)
        .collect();

    let fading: BTreeSet<&str> = net
        .lines
        .iter()
        .filter(|line| !keep.contains(&line.id))
        .flat_map(|line| line.strokes.iter())
        .map(String::as_str)
        .filter(|s| !kept.contains(s) && is_colour(s))
        .collect();

    let mut css = String::new();
    for s in fading {
        // Track carries `fill="none"`; a station's tick carries no fill at all.
        // Anything actually filled is a background area, which is not a line.
        css.push_str(&format!(
            "path[fill=\"none\"][stroke=\"{s}\"],path[stroke=\"{s}\"]:not([fill])\
             {{stroke:color-mix(in srgb,{s} {KEPT_PERCENT}%,{WASH})}}\n"
        ));
    }
    css
}

/// Whether a stroke value is safe to drop into a stylesheet verbatim.
///
/// Every colour syntax SVG allows — `#abc`, `rgb(…)`, `hsl(…)`, a keyword —
/// is made of these characters and needs no escaping. Anything else is left
/// unfaded rather than escaped: a line that keeps its colour is a cosmetic
/// loss, a stylesheet that can be broken out of is not.
fn is_colour(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '#' | '(' | ')' | ',' | '.' | '%' | '-' | ' ')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;

    /// Three lines over six stations. Red Line and Red Express share a stroke
    /// and differ only by Red Express's dashed overlay, which is exactly the
    /// pair CSS cannot separate. Blue Line's stroke is spelled with spaces,
    /// which the parser normalises away in `colour` but must not in `strokes`.
    const MAP: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0,0,14,10">
      <g>
        <path d="M0 0l3 0l0 3l-3 0z" fill="white" stroke="rgb(223,223,247)" stroke-width="0.05"/>
        <text font-size="0.4">Key to lines</text>
        <path d="M0.5 0.5l1 0" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <text>Red Line</text>
        <path d="M0.5 1l1 0" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M0.5 1l1 0" fill="none" stroke="white" stroke-width="0.1" stroke-dasharray="0.3 0.2"/>
        <text>Red Express</text>
        <path d="M0.5 1.5l1 0" fill="none" stroke="rgb(0, 0, 255)" stroke-width="0.2"/>
        <text>Blue Line</text>
      </g>
      <g>
        <path d="M1.5 1.5L2.5 1.5L3.5 1.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L2.5 3.5L3.5 3.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L2.5 3.5L3.5 3.5" fill="none" stroke="white" stroke-width="0.1" stroke-dasharray="0.3 0.2"/>
        <path d="M1.5 5.5L2.5 5.5L3.5 5.5" fill="none" stroke="rgb(0, 0, 255)" stroke-width="0.2"/>
      </g>
      <g>
        <path d="M1.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Ayane</text></g>
        <path d="M3.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Bellweather</text></g>
        <path d="M1.5 3.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Cortado</text></g>
        <path d="M3.5 3.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Dunsmuir</text></g>
        <path d="M1.5 5.275l0 0.45" stroke="rgb(0, 0, 255)" stroke-width="0.2"/>
        <g><text>Esker</text></g>
        <path d="M3.5 5.275l0 0.45" stroke="rgb(0, 0, 255)" stroke-width="0.2"/>
        <g><text>Fernbrake</text></g>
      </g>
    </svg>"##;

    fn net() -> Network {
        parse(MAP).expect("the fixture map parses")
    }

    fn id(net: &Network, name: &str) -> LineId {
        net.lines
            .iter()
            .find(|l| l.name == name)
            .unwrap_or_else(|| panic!("no line {name}"))
            .id
    }

    #[test]
    fn the_parser_keeps_the_spelling_the_file_used() {
        let net = net();
        let blue = net.line(id(&net, "Blue Line"));
        assert_eq!(blue.colour, "rgb(0,0,255)", "colour is normalised for matching");
        assert_eq!(blue.strokes, ["rgb(0, 0, 255)"], "strokes keep the file's spelling");
    }

    #[test]
    fn keeping_nothing_fades_every_line() {
        let css = wash_except(&net(), &[]);
        assert!(css.contains(r#"[stroke="hsl(0,100%,50%)"]"#), "{css}");
        assert!(css.contains(r#"[stroke="rgb(0, 0, 255)"]"#), "{css}");
    }

    #[test]
    fn a_kept_line_gets_no_rule_at_all() {
        let net = net();
        let css = wash_except(&net, &[id(&net, "Blue Line")]);
        assert!(!css.contains("rgb(0, 0, 255)"), "the journey must keep its colour: {css}");
        assert!(css.contains(r#"[stroke="hsl(0,100%,50%)"]"#), "the rest must fade: {css}");
    }

    #[test]
    fn a_colour_shared_with_the_journey_is_left_alone() {
        let net = net();
        // Red Express is a different line drawn in Red Line's colour, so a rule
        // fading it would fade the journey too.
        let css = wash_except(&net, &[id(&net, "Red Line")]);
        assert!(!css.contains("hsl(0,100%,50%)"), "{css}");
        assert!(css.contains(r#"[stroke="rgb(0, 0, 255)"]"#), "{css}");
    }

    #[test]
    fn selectors_reach_track_and_ticks_but_not_filled_areas() {
        let css = wash_except(&net(), &[]);
        let rule = css.lines().find(|l| l.contains("rgb(0, 0, 255)")).expect("a rule for blue");
        assert!(rule.contains(r#"path[fill="none"][stroke="rgb(0, 0, 255)"]"#), "track: {rule}");
        assert!(rule.contains(r#"path[stroke="rgb(0, 0, 255)"]:not([fill])"#), "ticks: {rule}");
        assert!(rule.contains(&format!("{KEPT_PERCENT}%,{WASH}")), "mix: {rule}");
    }

    #[test]
    fn one_rule_per_colour_however_many_lines_wear_it() {
        let css = wash_except(&net(), &[]);
        assert_eq!(css.lines().filter(|l| l.contains("hsl(0,100%,50%)")).count(), 1, "{css}");
        assert_eq!(css.lines().count(), 2, "two distinct colours, two rules: {css}");
    }

    #[test]
    fn nothing_that_could_escape_the_stylesheet_is_emitted() {
        for ok in ["#abc", "#aabbcc", "red", "rgb(1, 2, 3)", "hsl(87,100%,50%)", "rgb(0,0,0)"] {
            assert!(is_colour(ok), "{ok} is a colour");
        }
        for bad in [
            "",
            r#"red"]{}*{display:none}"#,
            "red</style><script>",
            "expression\\",
            "red;}body{}",
        ] {
            assert!(!is_colour(bad), "{bad:?} must not reach the stylesheet");
        }
    }
}
