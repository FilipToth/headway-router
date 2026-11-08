//! Reads a Tennessine Metro Designer SVG into a [`Network`].
//!
//! The format is documented in `docs/tennessine-svg-format.md`. The three facts
//! this leans on hardest:
//!
//! * every core track path is exactly one inter-station segment, so a segment's
//!   endpoints are its two stops and its interior vertices are not stops;
//! * a line's identity is its stroke colour plus whatever overlay strokes are
//!   drawn on top of it, which the key maps to a name;
//! * stations live on a half-integer grid, one per cell.

use crate::network::*;
use crate::path::{parse_path, polyline_length, PathError};
use crate::svg::parse_view_box;
use crate::Cell;
use roxmltree::{Document, Node, ParsingOptions};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Debug)]
pub enum ParseError {
    Xml(roxmltree::Error),
    NotAnSvg,
    NoViewBox,
    NoStationLayer,
    NoTrackLayer,
    BadPath { context: String, source: PathError },
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Xml(e) => write!(f, "this file is not valid XML: {e}"),
            Self::NotAnSvg => write!(f, "the root element is not <svg>"),
            Self::NoViewBox => write!(f, "the root <svg> has no usable viewBox"),
            Self::NoStationLayer => write!(
                f,
                "no station layer found: expected a group of markers paired with label groups"
            ),
            Self::NoTrackLayer => write!(f, "no track layer found: expected a group of unfilled polylines"),
            Self::BadPath { context, source } => write!(f, "{context}: {source}"),
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Xml(e) => Some(e),
            Self::BadPath { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<roxmltree::Error> for ParseError {
    fn from(e: roxmltree::Error) -> Self {
        Self::Xml(e)
    }
}

/// How a line is drawn: its stroke colour and the overlay stripes on top.
///
/// This is the whole of a line's identity in the file, which is why two lines
/// given the same colour and styling cannot be separated.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct StyleKey {
    colour: String,
    /// (colour, dash pattern) per overlay, sorted. Overlay width is left out
    /// deliberately: it distinguishes nothing and would make matching brittle.
    overlays: Vec<(String, Option<String>)>,
}

impl StyleKey {
    fn mode(&self) -> Mode {
        if self.overlays.iter().any(|(_, dash)| dash.is_some()) {
            Mode::CommuterRail
        } else if self.overlays.is_empty() {
            Mode::Metro
        } else {
            Mode::Tram
        }
    }
}

pub fn parse(svg: &str) -> Result<Network, ParseError> {
    // Tennessine writes an SVG 1.1 DOCTYPE, which roxmltree rejects unless asked.
    let doc = Document::parse_with_options(
        svg,
        ParsingOptions { allow_dtd: true, ..ParsingOptions::default() },
    )?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return Err(ParseError::NotAnSvg);
    }
    let view_box = root
        .attribute("viewBox")
        .and_then(parse_view_box)
        .ok_or(ParseError::NoViewBox)?;

    let mut warnings: Vec<Warning> = Vec::new();

    // ---- layers, by content: indices shift when optional layers are absent
    let layers: Vec<Node> = root
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "g")
        .collect();
    let key_layer = layers.iter().copied().find(is_key_layer);
    let station_layer = layers
        .iter()
        .copied()
        .find(is_station_layer)
        .ok_or(ParseError::NoStationLayer)?;

    // ---- the key: colour -> name(s)
    let legend = match key_layer {
        Some(layer) => read_key(layer),
        None => {
            warnings.push(Warning::NoKeyToLines);
            Vec::new()
        }
    };

    // ---- lines, one per distinct style
    let mut style_to_line: BTreeMap<StyleKey, LineId> = BTreeMap::new();
    let mut lines: Vec<Line> = Vec::new();
    for (name, style) in legend {
        match style_to_line.get(&style) {
            Some(existing) => {
                let line = &mut lines[existing.0];
                line.names.push(name);
                line.name = line.names.join(" / ");
            }
            None => {
                let id = LineId(lines.len());
                style_to_line.insert(style.clone(), id);
                lines.push(Line {
                    id,
                    name: name.clone(),
                    names: vec![name],
                    colour: style.colour.clone(),
                    strokes: Vec::new(),
                    mode: style.mode(),
                    shape: Shape::Path,
                    routable: true,
                    stops: Vec::new(),
                    edges: Vec::new(),
                });
            }
        }
    }
    for line in &lines {
        if line.names.len() > 1 {
            warnings.push(Warning::StyleSharedByLines {
                names: line.names.clone(),
                colour: line.colour.clone(),
            });
        }
    }

    // ---- stations
    let (mut stations, by_cell, station_warnings) = read_stations(station_layer)?;
    warnings.extend(station_warnings);
    if stations.is_empty() {
        return Err(ParseError::NoStationLayer);
    }

    // ---- track
    let mut track_layers: Vec<Node> = layers
        .iter()
        .copied()
        .filter(|l| Some(*l) != key_layer && *l != station_layer)
        .collect();
    track_layers.retain(|l| l.descendants().any(is_unfilled_path));
    if track_layers.is_empty() {
        return Err(ParseError::NoTrackLayer);
    }

    let paths: Vec<Node> = track_layers
        .iter()
        .flat_map(|l| l.descendants())
        .filter(|n| is_unfilled_path(*n))
        .collect();

    // A tram or commuter line is a wide coloured stroke with a narrower white
    // stripe drawn over identical path data. The widest path for a given `d` is
    // the track itself; the rest are decoration.
    let mut by_data: BTreeMap<&str, Vec<Node>> = BTreeMap::new();
    for p in paths {
        by_data.entry(p.attribute("d").unwrap_or("")).or_default().push(p);
    }

    let mut edges: Vec<Edge> = Vec::new();
    let from_key = lines.len();
    for (data, group) in by_data {
        let Some(core) = group.iter().copied().max_by(|a, b| {
            stroke_width(*a).partial_cmp(&stroke_width(*b)).unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            continue;
        };
        let mut overlays: Vec<(String, Option<String>)> = group
            .iter()
            .filter(|p| p.id() != core.id())
            .map(|p| {
                (
                    normalise_colour(inherited(*p, "stroke").unwrap_or("")),
                    inherited(*p, "stroke-dasharray").map(normalise_colour),
                )
            })
            .collect();
        overlays.sort();
        let style = StyleKey {
            colour: normalise_colour(inherited(core, "stroke").unwrap_or("")),
            overlays,
        };

        let subpaths = parse_path(data).map_err(|source| ParseError::BadPath {
            context: format!("track segment {data:.60}"),
            source,
        })?;
        let Some(points) = subpaths.into_iter().find(|s| s.len() >= 2) else {
            continue;
        };

        let line_id = *style_to_line.entry(style.clone()).or_insert_with(|| {
            let id = LineId(lines.len());
            lines.push(Line {
                id,
                name: format!("unnamed ({})", style.colour),
                names: vec![format!("unnamed ({})", style.colour)],
                colour: style.colour.clone(),
                strokes: Vec::new(),
                mode: style.mode(),
                shape: Shape::Path,
                routable: true,
                stops: Vec::new(),
                edges: Vec::new(),
            });
            id
        });

        // The stroke as written, not as normalised: a stylesheet matches the
        // attribute's exact text, so `rgb(0, 0, 255)` and `rgb(0,0,255)` are
        // different handles on the same colour.
        if let Some(raw) = inherited(core, "stroke") {
            let strokes = &mut lines[line_id.0].strokes;
            if !strokes.iter().any(|s| s == raw) {
                strokes.push(raw.to_string());
            }
        }

        let (first, last) = (points[0], points[points.len() - 1]);
        let (cell_a, cell_b) = (Cell::snap(first.0, first.1), Cell::snap(last.0, last.1));
        let (Some(a), Some(b)) = (by_cell.get(&cell_a).copied(), by_cell.get(&cell_b).copied())
        else {
            let missing = if by_cell.contains_key(&cell_a) { cell_b } else { cell_a };
            warnings.push(Warning::DanglingSegment {
                colour: style.colour.clone(),
                at: describe(missing),
            });
            continue;
        };
        if a == b {
            warnings.push(Warning::SelfLoop {
                line: lines[line_id.0].name.clone(),
                at: label_of(&stations, a),
            });
            continue;
        }

        let id = EdgeId(edges.len());
        edges.push(Edge {
            id,
            line: line_id,
            a,
            b,
            length: polyline_length(&points),
            polyline: points,
        });
        lines[line_id.0].edges.push(id);
        stations[a.0].edges.push(id);
        stations[b.0].edges.push(id);
    }
    for line in lines.iter().skip(from_key) {
        warnings.push(Warning::StyleNotInKey {
            colour: line.colour.clone(),
            segments: line.edges.len(),
        });
    }

    // ---- shape of each line, and its stop order
    for line in &mut lines {
        let (shape, stops, trouble) = walk_line(line, &edges, &stations);
        line.shape = shape;
        line.stops = stops;
        line.routable = matches!(shape, Shape::Path | Shape::Cycle) && !line.edges.is_empty();
        warnings.extend(trouble);
    }

    let mut network = Network {
        view_box,
        stations,
        lines,
        edges,
        warnings,
        by_cell,
        station_layer_at: station_layer.range().start,
    };

    // ---- consistency and reachability
    network.warnings.extend(tick_colour_disagreements(&network));
    for id in network.unreachable_stations() {
        if network.station(id).is_searchable() {
            network.warnings.push(Warning::StationUnreachable {
                station: network.label(id),
            });
        }
    }
    let pieces = network.components().len();
    if pieces > 1 {
        network.warnings.push(Warning::NetworkInPieces { pieces });
    }
    network.warnings.sort();
    network.warnings.dedup();
    Ok(network)
}

// ---------------------------------------------------------------- layers

fn is_key_layer(layer: &Node) -> bool {
    layer
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "text")
        .any(|t| text_of(t).trim() == "Key to lines")
}

/// The station layer is the one whose direct children include label groups:
/// `<g>` elements holding nothing but `<text>`.
fn is_station_layer(layer: &Node) -> bool {
    let mut groups = layer
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "g")
        .peekable();
    groups.peek().is_some()
        && groups.all(|g| {
            g.children()
                .filter(Node::is_element)
                .all(|c| c.tag_name().name() == "text")
        })
}

fn is_unfilled_path(n: Node) -> bool {
    n.is_element()
        && n.tag_name().name() == "path"
        && inherited(n, "fill").map(|f| f.eq_ignore_ascii_case("none")).unwrap_or(false)
        && n.attribute("d").is_some()
}

/// An attribute as it applies to this element, following presentation
/// attributes up through the enclosing groups.
fn inherited<'a>(node: Node<'a, 'a>, name: &str) -> Option<&'a str> {
    let mut at = Some(node);
    while let Some(n) = at {
        if let Some(v) = n.attribute(name) {
            return Some(v);
        }
        at = n.parent_element();
    }
    None
}

fn stroke_width(n: Node) -> f64 {
    inherited(n, "stroke-width").and_then(|w| w.trim().parse().ok()).unwrap_or(1.0)
}

/// The text content of an element. `descendants()` includes the element
/// itself, whose `text()` is its first child's, so only text nodes may count or
/// every label comes out doubled.
fn text_of(n: Node) -> String {
    n.descendants().filter(Node::is_text).filter_map(|d| d.text()).collect()
}

/// Colours and dash patterns are compared as strings, so whitespace and case
/// differences between the key and the track must not matter.
fn normalise_colour(raw: &str) -> String {
    raw.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase()
}

// ---------------------------------------------------------------- the key

/// Walks the key in document order: each sample stroke (plus any overlays drawn
/// over the same path data) is followed by the `<text>` naming that line.
fn read_key(layer: Node) -> Vec<(String, StyleKey)> {
    let mut out = Vec::new();
    let mut pending: Vec<Node> = Vec::new();
    for child in layer.children().filter(Node::is_element) {
        match child.tag_name().name() {
            // the key's white backing box is filled, the samples are not
            "path" if is_unfilled_path(child) => pending.push(child),
            "text" => {
                let name = text_of(child).trim().to_string();
                if name.is_empty() || name == "Key to lines" {
                    continue;
                }
                if let Some(style) = style_of_sample(&pending) {
                    out.push((name, style));
                }
                pending.clear();
            }
            _ => {}
        }
    }
    out
}

fn style_of_sample(sample: &[Node]) -> Option<StyleKey> {
    let core = sample.iter().copied().max_by(|a, b| {
        stroke_width(*a).partial_cmp(&stroke_width(*b)).unwrap_or(std::cmp::Ordering::Equal)
    })?;
    let mut overlays: Vec<(String, Option<String>)> = sample
        .iter()
        .filter(|p| p.id() != core.id())
        .map(|p| {
            (
                normalise_colour(inherited(*p, "stroke").unwrap_or("")),
                inherited(*p, "stroke-dasharray").map(normalise_colour),
            )
        })
        .collect();
    overlays.sort();
    Some(StyleKey {
        colour: normalise_colour(inherited(core, "stroke").unwrap_or("")),
        overlays,
    })
}

// ---------------------------------------------------------------- stations

type StationRead = (Vec<Station>, std::collections::HashMap<Cell, StationId>, Vec<Warning>);

/// The station layer is a flat run of marker(s) followed by a label group.
fn read_stations(layer: Node) -> Result<StationRead, ParseError> {
    let mut stations: Vec<Station> = Vec::new();
    let mut by_cell: std::collections::HashMap<Cell, StationId> = std::collections::HashMap::new();
    let mut warnings = Vec::new();
    let mut markers: Vec<Node> = Vec::new();

    for child in layer.children().filter(Node::is_element) {
        if child.tag_name().name() != "g" {
            markers.push(child);
            continue;
        }
        if markers.is_empty() {
            continue;
        }
        let label: String = child
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "text")
            .map(|t| text_of(t).trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            // Tennessine pre-wraps long names across two <text> lines
            .join(" ");

        let icons: Vec<Icon> = label.chars().filter_map(Icon::from_char).collect();
        let name = label
            .chars()
            .filter(|c| Icon::from_char(*c).is_none())
            .collect::<String>()
            .trim()
            .to_string();

        let (cell, marker) = marker_geometry(&markers)?;

        // Tennessine can emit one station's marker and label twice over. Merge
        // onto the station already in this cell rather than inventing a second
        // one, which would otherwise be orphaned from the graph.
        if let Some(existing) = by_cell.get(&cell).copied() {
            let held = stations[existing.0].name.clone();
            match (&held, name.is_empty()) {
                (Some(had), false) if *had != name => warnings.push(Warning::StationsShareACell {
                    at: describe(cell),
                    names: vec![had.clone(), name.clone()],
                }),
                _ => warnings.push(Warning::StationDrawnTwice {
                    station: held.clone().unwrap_or_else(|| describe(cell)),
                }),
            }
            let station = &mut stations[existing.0];
            if station.name.is_none() && !name.is_empty() {
                station.name = Some(name);
            }
            for icon in icons {
                if !station.icons.contains(&icon) {
                    station.icons.push(icon);
                }
            }
            if marker == Marker::Interchange {
                station.marker = Marker::Interchange;
            }
            markers.clear();
            continue;
        }

        let id = StationId(stations.len());
        by_cell.insert(cell, id);
        stations.push(Station {
            id,
            cell,
            name: (!name.is_empty()).then_some(name),
            icons,
            marker,
            edges: Vec::new(),
        });
        markers.clear();
    }
    Ok((stations, by_cell, warnings))
}

/// A station's cell, from a white interchange circle or from the centroid of
/// its tick marks.
fn marker_geometry(markers: &[Node]) -> Result<(Cell, Marker), ParseError> {
    if let Some(ellipse) = markers.iter().find(|m| m.tag_name().name() == "ellipse") {
        let num = |name: &str| ellipse.attribute(name).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        return Ok((Cell::snap(num("cx"), num("cy")), Marker::Interchange));
    }
    let mut sum = (0.0, 0.0);
    let mut count = 0.0;
    let mut ticks = 0;
    for m in markers.iter().filter(|m| m.tag_name().name() == "path") {
        let data = m.attribute("d").unwrap_or("");
        let subpaths = parse_path(data).map_err(|source| ParseError::BadPath {
            context: format!("station tick {data:.60}"),
            source,
        })?;
        ticks += 1;
        for point in subpaths.iter().flatten() {
            sum = (sum.0 + point.0, sum.1 + point.1);
            count += 1.0;
        }
    }
    if count == 0.0 {
        return Ok((Cell::snap(0.0, 0.0), Marker::Tick { ticks: 0 }));
    }
    Ok((Cell::snap(sum.0 / count, sum.1 / count), Marker::Tick { ticks }))
}

/// How many ticks a station was drawn with; `None` for interchange circles,
/// which carry no per-line information.
fn tick_count(station: &Station) -> Option<usize> {
    match station.marker {
        Marker::Tick { ticks } => Some(ticks),
        Marker::Interchange => None,
    }
}

// ---------------------------------------------------------------- shape

/// Classifies a line's own subgraph and, when it is usable, puts its stops in
/// travel order.
fn walk_line(line: &Line, edges: &[Edge], stations: &[Station]) -> (Shape, Vec<StationId>, Vec<Warning>) {
    let mut adjacency: BTreeMap<StationId, Vec<(StationId, EdgeId)>> = BTreeMap::new();
    for id in &line.edges {
        let e = &edges[id.0];
        adjacency.entry(e.a).or_default().push((e.b, e.id));
        adjacency.entry(e.b).or_default().push((e.a, e.id));
    }
    if adjacency.is_empty() {
        return (Shape::Disconnected, Vec::new(), Vec::new());
    }

    // one piece?
    let mut seen = BTreeSet::new();
    let mut pieces = 0;
    for start in adjacency.keys().copied() {
        if seen.contains(&start) {
            continue;
        }
        pieces += 1;
        let mut stack = vec![start];
        while let Some(at) = stack.pop() {
            if !seen.insert(at) {
                continue;
            }
            stack.extend(adjacency[&at].iter().map(|(n, _)| *n).filter(|n| !seen.contains(n)));
        }
    }
    if pieces > 1 {
        return (
            Shape::Disconnected,
            Vec::new(),
            vec![Warning::LineDisconnected { line: line.name.clone(), pieces }],
        );
    }

    let junctions: Vec<StationId> = adjacency
        .iter()
        .filter(|(_, next)| next.len() > 2)
        .map(|(at, _)| *at)
        .collect();
    if !junctions.is_empty() {
        return (
            Shape::Branched,
            Vec::new(),
            vec![Warning::LineBranches {
                line: line.name.clone(),
                at: junctions.iter().map(|id| label_of(stations, *id)).collect(),
            }],
        );
    }

    let termini: Vec<StationId> = adjacency
        .iter()
        .filter(|(_, next)| next.len() == 1)
        .map(|(at, _)| *at)
        .collect();
    let (shape, start) = match termini.len() {
        0 => (Shape::Cycle, *adjacency.keys().next().unwrap()),
        2 => (Shape::Path, termini[0]),
        _ => return (Shape::Branched, Vec::new(), Vec::new()),
    };

    let mut stops = vec![start];
    let mut used: BTreeSet<EdgeId> = BTreeSet::new();
    let mut at = start;
    while let Some((next, via)) = adjacency[&at].iter().find(|(_, e)| !used.contains(e)).copied() {
        used.insert(via);
        at = next;
        // a cycle arrives back where it began; list each stop once
        if at == start {
            break;
        }
        stops.push(at);
    }
    (shape, stops, Vec::new())
}

// ---------------------------------------------------------------- checks

/// Cross-checks the two layers against each other: the number of ticks at a
/// station should match the number of lines the track says calls there.
fn tick_colour_disagreements(net: &Network) -> Vec<Warning> {
    let mut out = Vec::new();
    for station in &net.stations {
        let Some(ticks) = tick_count(station) else { continue };
        let colours: BTreeSet<&str> = station
            .edges
            .iter()
            .map(|e| net.line(net.edge(*e).line).colour.as_str())
            .collect();
        // One tick per calling line, except that two lines sharing a style
        // share a colour and so cannot be counted separately.
        if ticks != colours.len() {
            out.push(Warning::TickColoursDisagree {
                station: net.label(station.id),
                ticks: vec![format!("{ticks} tick(s)")],
                edges: colours.iter().map(|c| c.to_string()).collect(),
            });
        }
    }
    out
}

fn label_of(stations: &[Station], id: StationId) -> String {
    let s = &stations[id.0];
    s.name.clone().unwrap_or_else(|| describe(s.cell))
}

fn describe(cell: Cell) -> String {
    let (x, y) = cell.centre();
    format!("({x}, {y})")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built map in Tennessine's shape, small enough to reason about.
    ///
    /// * Red Line — plain stroke, a three-stop path, starting at an interchange
    /// * Blue Loop — plain stroke, a closed triangle
    /// * Green Rail — dashed white overlay, so commuter rail; one unnamed stop
    ///   and one labelled with a station icon
    /// * Fork — branches at J, which Tennessine cannot really draw and we do
    ///   not route over
    const MAP: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0,0,20,12">
      <g>
        <path d="M0 0l3 0l0 3l-3 0z" fill="white" stroke="rgb(223,223,247)" stroke-width="0.05"/>
        <text x="0" y="0" font-size="0.4">Key to lines</text>
        <path d="M0.5 0.5l1 0" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <text x="2" y="0.5">Red Line</text>
        <path d="M0.5 1l1 0" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <text x="2" y="1">Blue Loop</text>
        <path d="M0.5 1.5l1 0" fill="none" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <path d="M0.5 1.5l1 0" fill="none" stroke="white" stroke-width="0.1" stroke-dasharray="0.3 0.2"/>
        <text x="2" y="1.5">Green Rail</text>
        <path d="M0.5 2l1 0" fill="none" stroke="hsl(300,100%,50%)" stroke-width="0.2"/>
        <text x="2" y="2">Fork</text>
      </g>
      <g>
        <path d="M1.5 1.5L2.5 1.5L3.5 1.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M3.5 1.5L4.5 1.5L5.5 1.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 5.5L2.5 5.5L3.5 5.5" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <path d="M3.5 5.5L3.5 6.5L3.5 7.5" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <path d="M3.5 7.5L2.5 6.5L1.5 5.5" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <path d="M7.5 1.5L8.5 1.5L9.5 1.5" fill="none" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <path d="M7.5 1.5L8.5 1.5L9.5 1.5" fill="none" stroke="white" stroke-width="0.1" stroke-dasharray="0.3 0.2"/>
        <path d="M1.5 9.5L2.5 9.5L3.5 9.5" fill="none" stroke="hsl(300,100%,50%)" stroke-width="0.2"/>
        <path d="M3.5 9.5L4.5 9.5L5.5 9.5" fill="none" stroke="hsl(300,100%,50%)" stroke-width="0.2"/>
        <path d="M3.5 9.5L3.5 10.5L3.5 11.5" fill="none" stroke="hsl(300,100%,50%)" stroke-width="0.2"/>
      </g>
      <g>
        <ellipse cx="1.5" cy="1.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Ayane</text></g>
        <path d="M3.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Bellweather</text><text>Cross</text></g>
        <path d="M5.5 1.15l0 0.7" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Cortado</text></g>
        <path d="M1.5 5.275l0 0.45" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <g><text>Dunsmuir</text></g>
        <path d="M3.5 5.275l0 0.45" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <g><text>Esker</text></g>
        <path d="M3.5 7.275l0 0.45" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <g><text>Fernbrake</text></g>
        <path d="M7.5 1.275l0 0.45" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <g><text>&#xe806;Grandhalt</text></g>
        <path d="M9.5 1.275l0 0.45" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <g></g>
        <path d="M1.5 9.275l0 0.45" stroke="hsl(300,100%,50%)" stroke-width="0.2"/>
        <g><text>Ingot</text></g>
        <path d="M3.5 9.275l0 0.45" stroke="hsl(300,100%,50%)" stroke-width="0.2"/>
        <g><text>Junction</text></g>
        <path d="M5.5 9.275l0 0.45" stroke="hsl(300,100%,50%)" stroke-width="0.2"/>
        <g><text>Kiln</text></g>
        <path d="M3.5 11.275l0 0.45" stroke="hsl(300,100%,50%)" stroke-width="0.2"/>
        <g><text>Lowmoor</text></g>
      </g>
    </svg>"##;

    fn net() -> Network {
        parse(MAP).expect("the fixture map parses")
    }

    fn line<'a>(net: &'a Network, name: &str) -> &'a Line {
        net.lines.iter().find(|l| l.name == name).unwrap_or_else(|| panic!("no line {name}"))
    }

    fn stops(net: &Network, name: &str) -> Vec<String> {
        line(net, name).stops.iter().map(|s| net.label(*s)).collect()
    }

    #[test]
    fn reads_the_key_and_infers_the_mode_from_the_overlay() {
        let net = net();
        assert_eq!(net.lines.len(), 4);
        assert_eq!(line(&net, "Red Line").mode, Mode::Metro);
        assert_eq!(line(&net, "Green Rail").mode, Mode::CommuterRail, "dashed overlay");
        assert_eq!(line(&net, "Red Line").colour, "hsl(0,100%,50%)");
        assert!(!net.warnings.contains(&Warning::NoKeyToLines));
    }

    #[test]
    fn reads_stations_with_their_markers_names_and_icons() {
        let net = net();
        assert_eq!(net.stations.len(), 12);
        assert_eq!(net.stations.iter().filter(|s| s.is_searchable()).count(), 11);

        let ayane = net.station(net.station_at(Cell::snap(1.5, 1.5)).unwrap());
        assert_eq!(ayane.name.as_deref(), Some("Ayane"));
        assert_eq!(ayane.marker, Marker::Interchange);

        // a two-line label is one name
        let b = net.station(net.station_at(Cell::snap(3.5, 1.5)).unwrap());
        assert_eq!(b.name.as_deref(), Some("Bellweather Cross"));
        assert_eq!(b.marker, Marker::Tick { ticks: 1 });

        // the icon glyph becomes a facility, not part of the name
        let g = net.station(net.station_at(Cell::snap(7.5, 1.5)).unwrap());
        assert_eq!(g.name.as_deref(), Some("Grandhalt"));
        assert_eq!(g.icons, vec![Icon::TrainStation]);

        // an empty label group is a real stop that simply cannot be searched
        let unnamed = net.station(net.station_at(Cell::snap(9.5, 1.5)).unwrap());
        assert!(unnamed.name.is_none() && !unnamed.edges.is_empty());
    }

    #[test]
    fn an_overlay_is_decoration_not_a_second_segment() {
        let net = net();
        // Green Rail is drawn as two paths over identical data, but is one track
        assert_eq!(line(&net, "Green Rail").edges.len(), 1);
        assert_eq!(net.edges.len(), 9, "ten paths, one of which is an overlay");
    }

    #[test]
    fn puts_a_paths_stops_in_travel_order() {
        let net = net();
        let l = line(&net, "Red Line");
        assert_eq!(l.shape, Shape::Path);
        assert!(l.routable);
        assert_eq!(stops(&net, "Red Line"), ["Ayane", "Bellweather Cross", "Cortado"]);
    }

    #[test]
    fn walks_a_loop_once_without_repeating_the_start() {
        let net = net();
        let l = line(&net, "Blue Loop");
        assert_eq!(l.shape, Shape::Cycle);
        assert_eq!(l.stops.len(), 3, "each stop once, start not repeated");
        assert_eq!(l.edges.len(), 3);
        assert!(l.routable);
    }

    #[test]
    fn a_branching_line_is_reported_and_left_out_of_routing() {
        let net = net();
        let l = line(&net, "Fork");
        assert_eq!(l.shape, Shape::Branched);
        assert!(!l.routable);
        assert!(l.stops.is_empty(), "no stop order exists for a shape we cannot walk");
        assert!(
            net.warnings.iter().any(|w| matches!(w, Warning::LineBranches { line, at }
                if line == "Fork" && at == &["Junction".to_string()])),
            "expected a branch warning naming Junction, got {:?}", net.warnings,
        );
    }

    #[test]
    fn excluding_a_line_strands_only_its_own_stops() {
        let net = net();
        let stranded: Vec<String> =
            net.unreachable_stations().iter().map(|s| net.label(*s)).collect();
        assert_eq!(stranded, ["Ingot", "Junction", "Kiln", "Lowmoor"]);
        // the rest of the map still routes, in two pieces here by construction
        assert!(net.components().iter().any(|p| p.len() == 3));
    }

    #[test]
    fn segments_connect_the_stations_at_their_ends() {
        let net = net();
        for e in &net.edges {
            let (first, last) = (e.polyline[0], e.polyline[e.polyline.len() - 1]);
            assert_eq!(net.station(e.a).cell, Cell::snap(first.0, first.1));
            assert_eq!(net.station(e.b).cell, Cell::snap(last.0, last.1));
            assert!(e.length > 0.0);
            assert_eq!(e.other(e.a), e.b);
        }
    }

    #[test]
    fn without_a_key_lines_survive_but_lose_their_names() {
        let stripped = MAP.replace("Key to lines", "");
        let net = parse(&stripped).expect("parses");
        assert!(net.warnings.contains(&Warning::NoKeyToLines));
        assert_eq!(net.lines.len(), 4, "still one line per distinct style");
        assert!(net.lines.iter().all(|l| l.name.starts_with("unnamed (")));
        assert_eq!(net.edges.len(), 9, "and the graph is unaffected");
    }

    #[test]
    fn a_station_drawn_twice_is_merged_not_duplicated() {
        // Tennessine does this; the second copy must not become an orphan node
        let doubled = MAP.replace(
            r#"<path d="M3.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Bellweather</text><text>Cross</text></g>"#,
            r#"<path d="M3.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Bellweather</text><text>Cross</text></g>
        <path d="M3.5 1.275l0 0.45" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <g><text>Bellweather</text><text>Cross</text></g>"#,
        );
        let net = parse(&doubled).expect("parses");
        assert_eq!(net.stations.len(), 12, "the copy is merged away");
        assert!(net.warnings.iter().any(|w| matches!(w, Warning::StationDrawnTwice { station }
            if station == "Bellweather Cross")));
        assert_eq!(stops(&net, "Red Line"), ["Ayane", "Bellweather Cross", "Cortado"]);
    }

    #[test]
    fn refuses_files_it_cannot_make_sense_of() {
        assert!(matches!(parse("<html></html>"), Err(ParseError::NotAnSvg)));
        assert!(matches!(
            parse(r#"<svg xmlns="http://www.w3.org/2000/svg"></svg>"#),
            Err(ParseError::NoViewBox)
        ));
        assert!(matches!(
            parse(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"></svg>"#),
            Err(ParseError::NoStationLayer)
        ));
    }
}
