//! Checks the parser against a real Tennessine export.
//!
//! The map is gitignored, so this skips when it is not there rather than
//! failing. It asserts invariants rather than counts, so re-exporting the map
//! does not break the suite.

use headway_core::network::Shape;
use headway_core::{fastest, parse, wash_except, Cell, Network};

fn real_map() -> Option<Network> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../muvit-amandleng.svg");
    let svg = std::fs::read_to_string(path).ok()?;
    Some(parse(&svg).expect("the real map parses"))
}

macro_rules! map_or_skip {
    () => {
        match real_map() {
            Some(net) => net,
            None => {
                eprintln!("skipping: no map at the workspace root");
                return;
            }
        }
    };
}

#[test]
fn every_station_holds_its_own_cell() {
    let net = map_or_skip!();
    assert_eq!(
        net.by_cell.len(),
        net.stations.len(),
        "a cell index shorter than the station list means two stations were merged onto one cell",
    );
    for s in &net.stations {
        assert_eq!(net.station_at(s.cell), Some(s.id));
    }
}

#[test]
fn every_segment_ends_on_the_stations_it_claims() {
    let net = map_or_skip!();
    assert!(!net.edges.is_empty());
    for e in &net.edges {
        let (first, last) = (e.polyline[0], e.polyline[e.polyline.len() - 1]);
        assert_eq!(net.station(e.a).cell, Cell::snap(first.0, first.1));
        assert_eq!(net.station(e.b).cell, Cell::snap(last.0, last.1));
        assert_ne!(e.a, e.b);
        assert!(e.length > 0.0, "a segment with no length cannot carry a journey");
        assert!(net.station(e.a).edges.contains(&e.id));
        assert!(net.station(e.b).edges.contains(&e.id));
    }
}

#[test]
fn a_stop_order_is_a_real_walk_along_the_line() {
    let net = map_or_skip!();
    let mut checked = 0;
    for line in net.routable_lines() {
        match line.shape {
            Shape::Path => assert_eq!(
                line.stops.len(),
                line.edges.len() + 1,
                "{} is a path, so it has one more stop than segment", line.name,
            ),
            Shape::Cycle => assert_eq!(
                line.stops.len(),
                line.edges.len(),
                "{} is a loop, so stops and segments match", line.name,
            ),
            other => panic!("{} is routable but {other:?}", line.name),
        }

        // every consecutive pair must actually be joined by a segment of this line
        let closing = (line.shape == Shape::Cycle).then(|| (line.stops[line.stops.len() - 1], line.stops[0]));
        let pairs = line.stops.windows(2).map(|w| (w[0], w[1])).chain(closing);
        for (from, to) in pairs {
            assert!(
                line.edges.iter().any(|id| {
                    let e = net.edge(*id);
                    (e.a == from && e.b == to) || (e.a == to && e.b == from)
                }),
                "{}: no segment between {} and {}",
                line.name,
                net.label(from),
                net.label(to),
            );
        }
        checked += 1;
    }
    assert!(checked > 1, "expected several routable lines, checked {checked}");
}

#[test]
fn every_stop_on_a_routable_line_can_be_reached() {
    let net = map_or_skip!();
    for line in net.routable_lines() {
        for stop in &line.stops {
            assert!(
                !net.unreachable_stations().contains(stop),
                "{} is a stop on {} yet counts as unreachable",
                net.label(*stop),
                line.name,
            );
        }
    }
}

#[test]
fn unnamed_stations_still_carry_track() {
    let net = map_or_skip!();
    let unnamed: Vec<_> = net.stations.iter().filter(|s| !s.is_searchable()).collect();
    assert!(!unnamed.is_empty(), "this map has unlabelled stops");
    for s in &unnamed {
        assert!(!s.edges.is_empty(), "an unnamed station is still a real stop on a line");
    }
    assert!(
        net.search_index().values().all(|ids| !ids.is_empty()),
        "the search index only holds named stations",
    );
}

#[test]
fn parsing_is_deterministic() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../muvit-amandleng.svg");
    let Ok(svg) = std::fs::read_to_string(path) else {
        eprintln!("skipping: no map at the workspace root");
        return;
    };
    let (a, b) = (parse(&svg).unwrap(), parse(&svg).unwrap());
    assert_eq!(a.warnings, b.warnings, "warnings must not depend on hash order");
    let names = |n: &Network| n.lines.iter().map(|l| l.name.clone()).collect::<Vec<_>>();
    assert_eq!(names(&a), names(&b));
    let order = |n: &Network| {
        n.lines.iter().map(|l| l.stops.iter().map(|s| n.label(*s)).collect::<Vec<_>>()).collect::<Vec<_>>()
    };
    assert_eq!(order(&a), order(&b), "stop order must not depend on hash order");
}

#[test]
fn every_drawn_line_can_be_addressed_in_a_stylesheet() {
    let net = map_or_skip!();
    for line in net.lines.iter().filter(|l| !l.edges.is_empty()) {
        assert!(
            !line.strokes.is_empty(),
            "{} has {} segments but no stroke to match on",
            line.name,
            line.edges.len(),
        );
        for stroke in &line.strokes {
            let fades = wash_except(&net, &[]);
            assert!(
                fades.contains(&format!("[stroke=\"{stroke}\"]")),
                "{} is drawn in {stroke}, which no rule reaches",
                line.name,
            );
        }
    }
}

#[test]
fn a_journey_keeps_its_colour_while_the_rest_of_the_network_fades() {
    let net = map_or_skip!();
    let from = net.station_named("Zamalek").expect("Zamalek is on the map");
    let to = net.station_named("Xiguan").expect("Xiguan is on the map");
    let journey = fastest(&net, from, to).expect("Zamalek reaches Xiguan");

    let ridden: Vec<_> = journey.legs.iter().map(|l| l.line).collect();
    let css = wash_except(&net, &ridden);
    assert!(!css.is_empty(), "something must fade");

    for id in &ridden {
        for stroke in &net.line(*id).strokes {
            assert!(
                !css.contains(stroke.as_str()),
                "{} is ridden and must keep its colour",
                net.line(*id).name,
            );
        }
    }

    // Everything else fades, except where it is drawn in a ridden colour.
    let spared: Vec<&str> = ridden
        .iter()
        .flat_map(|id| net.line(*id).strokes.iter())
        .map(String::as_str)
        .collect();
    for line in net.lines.iter().filter(|l| !ridden.contains(&l.id)) {
        for stroke in &line.strokes {
            assert!(
                css.contains(stroke.as_str()) || spared.contains(&stroke.as_str()),
                "{} stays saturated for no reason",
                line.name,
            );
        }
    }
}
