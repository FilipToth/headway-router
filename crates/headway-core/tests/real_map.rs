//! Checks the parser against a real Tennessine export.
//!
//! The map is gitignored, so this skips when it is not there rather than
//! failing. It asserts invariants rather than counts, so re-exporting the map
//! does not break the suite.

use headway_core::network::Shape;
use headway_core::{fade, fastest, parse, root_inner_range, trace, Cell, Network, Scale};

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
    let css = fade(&net);
    for line in net.lines.iter().filter(|l| !l.edges.is_empty()) {
        assert!(
            !line.strokes.is_empty(),
            "{} has {} segments but no stroke to match on",
            line.name,
            line.edges.len(),
        );
        for stroke in &line.strokes {
            assert!(
                css.contains(&format!("[stroke=\"{stroke}\"]")),
                "{} is drawn in {stroke}, which no rule reaches",
                line.name,
            );
        }
    }
}

#[test]
fn the_route_layer_slots_between_the_track_and_the_markers() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../muvit-amandleng.svg");
    let Ok(svg) = std::fs::read_to_string(path) else {
        eprintln!("skipping: no map at the workspace root");
        return;
    };
    let net = parse(&svg).expect("the real map parses");
    let body = root_inner_range(&svg).expect("a root <svg>");
    let at = net.station_layer_at;
    assert!(body.contains(&at), "the split must land inside the body");
    assert!(svg[at..].starts_with("<g"), "it must land on a layer's opening tag");

    // Everything the route is drawn over is behind it; the markers and labels
    // that must stay on top are ahead of it.
    let (under, over) = (&svg[body.start..at], &svg[at..body.end]);
    assert!(under.contains("fill=\"none\""), "track belongs under the route");
    assert!(under.contains("Key to lines"), "so does the key");
    assert!(over.contains("<ellipse"), "interchange circles belong over it");
    assert!(over.contains("<text"), "so do station labels");
    assert!(!over.contains("fill=\"none\""), "no track should be left above it");
}

#[test]
fn a_journey_is_traced_over_the_track_it_rides() {
    let net = map_or_skip!();
    let from = net.station_named("Zamalek").expect("Zamalek is on the map");
    let to = net.station_named("Xiguan").expect("Xiguan is on the map");
    let journey = fastest(&net, from, to).expect("Zamalek reaches Xiguan");
    let svg = trace(&net, &journey);

    assert_eq!(
        svg.matches("<path").count(),
        journey.legs.len() * 3,
        "a halo, a casing and a colour per leg",
    );
    assert_eq!(
        svg.matches("<circle").count(),
        journey.legs.iter().map(|l| l.stops.len()).sum::<usize>(),
        "a dot at every stop",
    );
    for leg in &journey.legs {
        let points = leg.polyline(&net);
        assert!(points.len() > leg.edges.len(), "one joined polyline per leg");

        let stops = leg.stop_points(&net);
        assert_eq!(stops.len(), leg.stops.len(), "a drawn point for every stop");
        assert_eq!(stops[0], points[0], "the first stop starts the polyline");
        assert_eq!(stops[stops.len() - 1], points[points.len() - 1], "the last ends it");

        // Drawn track sits in its corridor slot, offset from the cell centre by
        // multiples of 0.095 — near its station, never exactly on it.
        for (drawn, stop) in stops.iter().zip(&leg.stops) {
            let (cx, cy) = net.station(*stop).cell.centre();
            let off = ((drawn.0 - cx).powi(2) + (drawn.1 - cy).powi(2)).sqrt();
            assert!(
                off < 0.5,
                "{} is drawn {off:.3} units from its own cell",
                net.label(*stop),
            );
        }
    }

    // The fade must reach every line, so the trace is the only colour left.
    let css = fade(&net);
    for line in net.lines.iter().filter(|l| !l.edges.is_empty()) {
        for stroke in &line.strokes {
            assert!(css.contains(stroke.as_str()), "{} stays saturated", line.name);
        }
    }
}

#[test]
fn a_fresh_scale_reaches_every_station_on_the_map() {
    let net = map_or_skip!();
    let scale = Scale::for_network(&net);
    let vb = net.view_box;

    assert!(
        (vb.min_x..vb.min_x + vb.width).contains(&scale.centre.0)
            && (vb.min_y..vb.min_y + vb.height).contains(&scale.centre.1),
        "the default centre {:?} fell outside the map",
        scale.centre,
    );
    for s in &net.stations {
        let (x, y) = s.cell.centre();
        let r = (x - scale.centre.0).hypot(y - scale.centre.1);
        assert!(
            r <= scale.reach() + 1e-9,
            "{} sits {r:.1} units out, past the profile's {:.1} unit reach",
            net.label(s.id),
            scale.reach(),
        );
    }
}

#[test]
fn a_journey_measures_as_its_profile_says_it_should() {
    let net = map_or_skip!();
    let from = net.station_named("Zamalek").expect("Zamalek is on the map");
    let to = net.station_named("Xiguan").expect("Xiguan is on the map");
    let journey = fastest(&net, from, to).expect("Zamalek reaches Xiguan");

    // Measured one for one, the real distance is the drawn distance: the check
    // that nothing has silently crept into the measurement itself.
    let fresh = Scale::for_network(&net);
    let plain = Scale::from_points(fresh.centre, vec![(fresh.reach(), fresh.reach())]);
    let drawn = journey.distance(&net);
    let measured = plain.journey_km(&net, &journey);
    assert!(
        (measured - drawn).abs() < 1e-6,
        "{measured} km measured against {drawn} units of track",
    );
    let legs: f64 = journey.legs.iter().map(|l| plain.leg_km(&net, l)).sum();
    assert!((legs - measured).abs() < 1e-9, "the legs must add up to the journey");

    // A fresh profile is a shape, not a measurement: a map unit is worth far
    // less in the middle than at the rim, and the furthest station still lands
    // at roughly its drawn distance in km.
    let (middle, rim) = (fresh.km_per_unit(0.0), fresh.km_per_unit(fresh.reach()));
    assert!(rim > middle * 3.0, "{rim} km/unit at the rim against {middle} in the middle");
    assert!(
        (fresh.real_radius(fresh.reach()) - fresh.reach()).abs() < fresh.reach() * 0.1,
        "a fresh map should still be about {} km across, not {}",
        fresh.reach(),
        fresh.real_radius(fresh.reach()),
    );

    // A profile that says the drawn centre is stretched has to come out
    // shorter — that is the whole point of the setting.
    let squeezed = Scale::from_points(
        plain.centre,
        vec![(20.0, 4.0), (plain.reach(), plain.reach() * 0.6)],
    );
    let shorter = squeezed.journey_km(&net, &journey);
    assert!(shorter < measured * 0.8, "{shorter} km should undercut {measured} km");
    assert!(shorter > 0.0, "a journey still covers ground");
}
