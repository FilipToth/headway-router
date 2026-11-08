//! Making one journey the only thing on a crowded map.
//!
//! A Tennessine map of any size is dense — dozens of lines, hundreds of
//! stations, parallel corridors 0.095 units apart. Tinting the route alone is
//! not enough to find it, so this does both halves of the job: [`fade`] pushes
//! the whole network back to a pale diagram, and [`trace`] draws the journey
//! over it as new, bolder geometry.

use crate::network::{Line, Network};
use crate::route::Journey;
use std::collections::BTreeSet;

/// The grey a faded line is mixed towards — close to Tennessine's paper, so
/// the network reads as a watermark under the route.
pub const FADE_TARGET: &str = "#d2d2d9";

/// How much of a line's own colour survives the fade, as a CSS percentage.
/// Enough to tell a faded red from a faded green when you look for it, not
/// enough to catch the eye while the route is on screen.
pub const FADE_KEPT_PERCENT: u8 = 12;

/// Width of the route itself, against track drawn at 0.2.
pub const TRACE_WIDTH: f64 = 0.3;
/// A dark edge under the route's colour, and the width it is drawn at.
///
/// Not decoration: Tennessine's palette runs to 70% lightness, and a route in
/// one of those colours is pale ink on pale paper however faded the rest of the
/// map is. The edge makes every route read the same regardless of its line.
pub const CASING: &str = "#23232b";
pub const CASING_WIDTH: f64 = 0.4;
/// A translucent band either side of the route, like a highlighter stroke.
/// Wide enough to claim the whole corridor, see-through enough that the lines
/// it covers are still there.
pub const HALO_WIDTH: f64 = 0.9;
pub const HALO_OPACITY: f64 = 0.3;
/// A dot at each stop along the route, since the station's own tick is faded
/// and would otherwise be lost under the route. Ringed in [`CASING`] rather
/// than the line's colour, for the same reason the casing exists.
pub const STOP_RADIUS: f64 = 0.13;
pub const STOP_RING: f64 = 0.075;

/// Class on the route's own strokes, and the one piece of coupling between the
/// two halves of this module.
///
/// [`fade`] addresses a line by its colour, and [`trace`] draws the route in
/// that same colour — so without something to tell them apart the stylesheet
/// would fade the very thing it exists to set off. Tennessine writes no `class`
/// anywhere, so this marks ours and the fade rules step around it.
pub const LEG_CLASS: &str = "leg";

// Checked on every build, because getting these wrong is the whole bug: a
// route drawn at Tennessine's own 0.2 track width is not a highlight, and a
// halo must be wide enough to claim its corridor and sheer enough to let the
// map show through.
const _: () = assert!(TRACE_WIDTH > 0.2);
const _: () = assert!(CASING_WIDTH > TRACE_WIDTH);
const _: () = assert!(HALO_WIDTH > CASING_WIDTH * 2.0);
const _: () = assert!(HALO_OPACITY > 0.0 && HALO_OPACITY < 1.0);
const _: () = assert!(FADE_KEPT_PERCENT > 0 && FADE_KEPT_PERCENT < 100);

/// A stylesheet that fades every line on the map to a pale tint of its own
/// colour, leaving nothing saturated for [`trace`] to compete with.
///
/// Lines are addressed by their stroke exactly as the file spells it, because
/// in Tennessine's markup that is the whole of a line's identity — there is no
/// class or id to hang a rule on, and tagging the paths would mean rewriting
/// half a megabyte of SVG on every query. Two things follow:
///
/// * Every line fades, the ridden ones included. The route is separate
///   geometry, so it does not need its line spared — which also means two
///   lines sharing a colour cause no trouble here.
/// * The key's swatches fade along with the lines they stand for.
///
/// The route drawn by [`trace`] is exempted by [`LEG_CLASS`], because it wears
/// its line's colour and would otherwise fade with it.
///
/// Station ticks are drawn in their line's colour and so fade with it; an
/// interchange's circle is black on white and never fades.
pub fn fade(net: &Network) -> String {
    let colours: BTreeSet<&str> = net
        .lines
        .iter()
        .flat_map(|line| line.strokes.iter())
        .map(String::as_str)
        .filter(|s| is_colour(s))
        .collect();

    let mut css = String::new();
    for c in colours {
        // Track carries `fill="none"`; a station's tick carries no fill at all.
        // Anything actually filled is a background area, which is not a line.
        css.push_str(&format!(
            "path[fill=\"none\"][stroke=\"{c}\"]:not(.{LEG_CLASS}),\
             path[stroke=\"{c}\"]:not([fill])\
             {{stroke:color-mix(in srgb,{c} {FADE_KEPT_PERCENT}%,{FADE_TARGET})}}\n"
        ));
    }
    css
}

/// The journey drawn as SVG, to be mounted over the track and under the station
/// markers (see [`Network::station_layer_at`]).
///
/// Every leg gets a translucent halo, a dark casing, a bold stroke in its
/// line's colour, and a dot at each stop — placed where the track is drawn
/// rather than where the station sits, so it lands on the route and not beside
/// it. The halos all go down before any casing and every casing before any
/// colour, so that where two legs meet at a transfer neither one's backing
/// washes over the other's stroke.
pub fn trace(net: &Network, journey: &Journey) -> String {
    let legs: Vec<(String, String)> = journey
        .legs
        .iter()
        .map(|leg| (colour_of(net.line(leg.line)), path_d(&leg.polyline(net))))
        .collect();

    let mut svg = String::new();
    for (colour, d) in &legs {
        svg.push_str(&stroke(d, colour, HALO_WIDTH, Some(HALO_OPACITY)));
    }
    for (_, d) in &legs {
        svg.push_str(&stroke(d, CASING, CASING_WIDTH, None));
    }
    for (colour, d) in &legs {
        svg.push_str(&stroke(d, colour, TRACE_WIDTH, None));
    }
    for leg in &journey.legs {
        for (x, y) in leg.stop_points(net) {
            svg.push_str(&format!(
                "<circle class=\"{LEG_CLASS}\" cx=\"{x:.4}\" cy=\"{y:.4}\" \
                 r=\"{STOP_RADIUS}\" fill=\"#fff\" \
                 stroke=\"{CASING}\" stroke-width=\"{STOP_RING}\"/>"
            ));
        }
    }
    svg
}

/// One stroke along a leg. Round caps and joins are what let the three passes
/// stack cleanly, and what closes the 0.095 corridor offset where one leg hands
/// over to the next at a transfer.
fn stroke(d: &str, colour: &str, width: f64, opacity: Option<f64>) -> String {
    let opacity = match opacity {
        Some(o) => format!(" stroke-opacity=\"{o}\""),
        None => String::new(),
    };
    format!(
        "<path class=\"{LEG_CLASS}\" d=\"{d}\" fill=\"none\" stroke=\"{colour}\" \
         stroke-width=\"{width}\"{opacity} \
         stroke-linecap=\"round\" stroke-linejoin=\"round\"/>"
    )
}

/// An SVG `d` for a polyline, at the precision the grid actually uses.
fn path_d(points: &[(f64, f64)]) -> String {
    let mut d = String::new();
    for (i, (x, y)) in points.iter().enumerate() {
        d.push(if i == 0 { 'M' } else { 'L' });
        d.push_str(&format!("{x:.4} {y:.4}"));
        if i + 1 < points.len() {
            d.push(' ');
        }
    }
    d
}

/// A line's colour, in the file's own spelling where that is safe to re-emit.
///
/// The fallback is a visible grey rather than nothing: a route drawn in the
/// wrong colour is still a route, an unreadable map is not.
fn colour_of(line: &Line) -> String {
    line.strokes
        .iter()
        .chain(std::iter::once(&line.colour))
        .find(|s| is_colour(s))
        .cloned()
        .unwrap_or_else(|| "#444".to_string())
}

/// Whether a colour from the file is safe to re-emit verbatim, into either a
/// stylesheet selector or an SVG attribute.
///
/// Every colour syntax SVG allows — `#abc`, `rgb(…)`, `hsl(…)`, a keyword — is
/// made of these characters and needs no escaping. Anything else is dropped
/// rather than escaped: a line that keeps its colour, or a route drawn in grey,
/// is a cosmetic loss; markup that can be broken out of is not.
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
    use crate::network::LineId;
    use crate::{fastest, parse};

    /// Three lines over six stations. Red Line and Red Express share a stroke
    /// and differ only by Red Express's dashed overlay — the pair CSS cannot
    /// separate, which is why nothing is spared the fade. Blue Line's stroke is
    /// spelled with spaces, which the parser normalises away in `colour` but
    /// must not in `strokes`. Red Line's two segments are drawn in opposite
    /// directions, so a leg has to flip one to trace it.
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
        <path d="M5.5 1.5L4.5 1.5L3.5 1.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L2.5 3.5L3.5 3.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L2.5 3.5L3.5 3.5" fill="none" stroke="white" stroke-width="0.1" stroke-dasharray="0.3 0.2"/>
        <path d="M1.5 5.5L2.5 5.5L3.5 5.5" fill="none" stroke="rgb(0, 0, 255)" stroke-width="0.2"/>
      </g>
      <g>
        <path d="M1.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Ayane</text></g>
        <path d="M3.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Bellweather</text></g>
        <path d="M5.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Cortado</text></g>
        <path d="M1.5 3.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Quarry</text></g>
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

    fn ride(net: &Network, from: &str, to: &str) -> Journey {
        let a = net.station_named(from).unwrap_or_else(|| panic!("no station {from}"));
        let b = net.station_named(to).unwrap_or_else(|| panic!("no station {to}"));
        fastest(net, a, b).expect("the fixture stations connect")
    }

    // ------------------------------------------------------------ the fade

    #[test]
    fn the_parser_keeps_the_spelling_the_file_used() {
        let net = net();
        let blue = net.line(id(&net, "Blue Line"));
        assert_eq!(blue.colour, "rgb(0,0,255)", "colour is normalised for matching");
        assert_eq!(blue.strokes, ["rgb(0, 0, 255)"], "strokes keep the file's spelling");
    }

    #[test]
    fn every_line_fades_including_the_ones_the_route_uses() {
        let css = fade(&net());
        assert!(css.contains(r#"[stroke="hsl(0,100%,50%)"]"#), "{css}");
        assert!(css.contains(r#"[stroke="rgb(0, 0, 255)"]"#), "{css}");
    }

    #[test]
    fn selectors_reach_track_and_ticks_but_not_filled_areas() {
        let css = fade(&net());
        let rule = css.lines().find(|l| l.contains("rgb(0, 0, 255)")).expect("a rule for blue");
        assert!(rule.contains(r#"path[fill="none"][stroke="rgb(0, 0, 255)"]"#), "track: {rule}");
        assert!(rule.contains(r#"path[stroke="rgb(0, 0, 255)"]:not([fill])"#), "ticks: {rule}");
        assert!(rule.contains(&format!("{FADE_KEPT_PERCENT}%,{FADE_TARGET}")), "mix: {rule}");
    }

    #[test]
    fn one_rule_per_colour_however_many_lines_wear_it() {
        let css = fade(&net());
        assert_eq!(css.lines().filter(|l| l.contains("hsl(0,100%,50%)")).count(), 1, "{css}");
        assert_eq!(css.lines().count(), 2, "two distinct colours, two rules: {css}");
    }

    // ----------------------------------------------------------- the trace

    #[test]
    fn a_leg_is_one_polyline_however_its_segments_were_drawn() {
        let net = net();
        // Ayane -> Cortado spans two Red Line segments drawn towards each other.
        let journey = ride(&net, "Ayane", "Cortado");
        let leg = &journey.legs[0];
        assert_eq!(leg.edges.len(), 2, "two segments");
        let points = leg.polyline(&net);
        assert_eq!(points.first(), Some(&(1.5, 1.5)), "starts where we board");
        assert_eq!(points.last(), Some(&(5.5, 1.5)), "ends where we alight");
        assert_eq!(points.len(), 5, "five vertices, the shared stop counted once");
        for pair in points.windows(2) {
            assert!(pair[0].0 < pair[1].0, "must march one way: {points:?}");
        }
    }

    #[test]
    fn the_trace_stacks_halo_casing_colour_and_a_dot_per_stop() {
        let net = net();
        let journey = ride(&net, "Ayane", "Cortado");
        let svg = trace(&net, &journey);
        assert_eq!(svg.matches("<path").count(), 3, "halo, casing, colour: {svg}");
        assert_eq!(svg.matches("<circle").count(), 3, "a dot at each of 3 stops: {svg}");

        let at = |w: f64| svg.find(&format!("stroke-width=\"{w}\"")).expect("a stroke");
        assert!(at(HALO_WIDTH) < at(CASING_WIDTH), "halo under casing: {svg}");
        assert!(at(CASING_WIDTH) < at(TRACE_WIDTH), "casing under colour: {svg}");
        assert!(
            svg.contains(&format!("stroke-opacity=\"{HALO_OPACITY}\"")),
            "only the halo is translucent: {svg}",
        );
        assert_eq!(svg.matches("stroke-opacity").count(), 1, "{svg}");
    }

    #[test]
    fn a_pale_line_is_still_readable_because_of_the_casing() {
        let net = net();
        let journey = ride(&net, "Ayane", "Cortado");
        let svg = trace(&net, &journey);
        assert!(svg.contains(&format!("stroke=\"{CASING}\"")), "a dark edge: {svg}");
        // The dots are ringed in the casing too, not in the line's colour.
        let dot = svg.split("<circle").nth(1).expect("a dot");
        assert!(dot.contains(&format!("stroke=\"{CASING}\"")), "{dot}");
    }

    #[test]
    fn the_fade_steps_around_the_route_it_is_drawn_against() {
        let net = net();
        let journey = ride(&net, "Esker", "Fernbrake");
        let drawn = trace(&net, &journey);
        let css = fade(&net);

        // The route is drawn in its line's colour, which the fade also matches.
        let colour = &net.line(journey.legs[0].line).strokes[0];
        assert!(drawn.contains(&format!("stroke=\"{colour}\"")), "{drawn}");
        assert!(css.contains(&format!("[stroke=\"{colour}\"]")), "{css}");

        // So every stroke of the route must carry the class the fade excludes.
        assert_eq!(
            drawn.matches(&format!("class=\"{LEG_CLASS}\"")).count(),
            drawn.matches("<path").count() + drawn.matches("<circle").count(),
            "every drawn element needs the class: {drawn}",
        );
        let rule = css.lines().find(|l| l.contains(colour.as_str())).expect("a rule");
        assert!(rule.contains(&format!(":not(.{LEG_CLASS})")), "{rule}");
    }

    #[test]
    fn the_trace_follows_the_track_it_rides() {
        let net = net();
        let journey = ride(&net, "Esker", "Fernbrake");
        let svg = trace(&net, &journey);
        assert!(svg.contains(r#"stroke="rgb(0, 0, 255)""#), "in the line's own colour: {svg}");
        assert!(svg.contains("M1.5000 5.5000 L2.5000 5.5000 L3.5000 5.5000"), "{svg}");
    }

    #[test]
    fn nothing_that_could_escape_the_markup_is_emitted() {
        for ok in ["#abc", "#aabbcc", "red", "rgb(1, 2, 3)", "hsl(87,100%,50%)", "rgb(0,0,0)"] {
            assert!(is_colour(ok), "{ok} is a colour");
        }
        for bad in [
            "",
            r#"red"]{}*{display:none}"#,
            "red</style><script>",
            "expression\\",
            "red;}body{}",
            r#"red" onload="alert(1)"#,
        ] {
            assert!(!is_colour(bad), "{bad:?} must not reach the markup");
        }
    }

    #[test]
    fn a_colour_the_guard_rejects_still_draws_a_route() {
        let line = Line {
            id: LineId(0),
            name: "Hostile".into(),
            names: vec!["Hostile".into()],
            colour: r#"red" onload="alert(1)"#.into(),
            strokes: vec![r#"red"><script>"#.into()],
            mode: crate::Mode::Metro,
            shape: crate::Shape::Path,
            routable: true,
            stops: Vec::new(),
            edges: Vec::new(),
        };
        let drawn = colour_of(&line);
        assert!(is_colour(&drawn), "{drawn:?} must be safe to re-emit");
        assert_eq!(drawn, "#444", "and visible, not empty");
    }
}
