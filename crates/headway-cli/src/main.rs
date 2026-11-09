//! Drives the parser without a browser, which is how the parser gets checked.
//!
//!     headway <map.svg> [--lines] [--stations] [--warnings] [--line <name>]
//!                       [--route <from> <to>] [--find <query>] [--paint]
//!                       [--profile <units>:<km>,...] [--centre <x>,<y>]
//!                       [--headway <minutes>] [--walk <seconds>]
//!                       [--off <category>]...

use headway_core::network::{Marker, Shape};
use headway_core::{
    fade, options, parse, tag, trace, Choice, Network, Scale, Services, StationId, Timing,
};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "usage: headway <map.svg> [--lines] [--stations] [--warnings] [--line <name>] \
             [--route <from> <to>] [--find <query>] [--paint] \
             [--profile <units>:<km>,...] [--centre <x>,<y>] [--headway <minutes>] \
             [--walk <seconds>] [--off <category>]..."
        );
        return ExitCode::from(2);
    }

    let path = &args[0];
    let want = |flag: &str| args.iter().any(|a| a == flag);
    let one_line = args
        .iter()
        .position(|a| a == "--line")
        .and_then(|i| args.get(i + 1))
        .cloned();

    let svg = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let net = match parse(&svg) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("cannot parse {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(i) = args.iter().position(|a| a == "--find") {
        let Some(query) = args.get(i + 1) else {
            eprintln!("--find needs something to look for");
            return ExitCode::from(2);
        };
        println!("{query:?} suggests:");
        for id in net.search(query, 7) {
            println!("    {}", net.label(id));
        }
        return ExitCode::SUCCESS;
    }

    if let Some(i) = args.iter().position(|a| a == "--route") {
        let (Some(from), Some(to)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("--route needs two station names");
            return ExitCode::from(2);
        };
        let measure = scale(&net, &args);
        let services = seeded(&net, &args);
        let timing = Timing::new(&net, &measure, &services);
        return route(&timing, &measure, &services, from, to, want("--paint"));
    }

    summary(&net, path);
    if want("--lines") || one_line.is_some() {
        lines(&net, one_line.as_deref());
    }
    if want("--stations") {
        stations(&net);
    }
    if want("--warnings") || !net.warnings.is_empty() {
        warnings(&net);
    }
    ExitCode::SUCCESS
}

/// The distance model to report in, which the web app's settings panel edits.
///
/// `--profile 20:4,130:78` gives the control points as `units:km` from the
/// centre, and `--centre -3.5,12` moves the centre they are measured from.
/// Without them the map gets the default profile: centred on its stations,
/// worth little per unit in the middle and several times that at the rim.
fn scale(net: &Network, args: &[String]) -> Scale {
    let mut base = Scale::for_network(net);
    if let Some(spec) = args.iter().position(|a| a == "--centre").and_then(|i| args.get(i + 1)) {
        match spec.split_once(',').and_then(|(x, y)| {
            Some((x.trim().parse::<f64>().ok()?, y.trim().parse::<f64>().ok()?))
        }) {
            Some(at) => base.centre = at,
            None => eprintln!("--centre wants a pair like -3.5,12; ignoring {spec:?}"),
        }
    }
    let Some(spec) = args.iter().position(|a| a == "--profile").and_then(|i| args.get(i + 1))
    else {
        return base;
    };
    let points: Vec<(f64, f64)> = spec
        .split(',')
        .filter_map(|pair| {
            let (r, km) = pair.split_once(':')?;
            Some((r.trim().parse().ok()?, km.trim().parse().ok()?))
        })
        .collect();
    if points.is_empty() {
        eprintln!("--profile wants pairs like 20:4,130:78; ignoring {spec:?}");
        return base;
    }
    Scale::from_points(base.centre, points)
}

/// The map's service categories, with `--headway 12` putting every one of
/// them on a twelve minute service and `--walk 90` lengthening the change —
/// enough to see what frequency does to a route without a browser.
///
/// `--off tram`, repeatable, is the routing panel's switches: it takes a
/// category out of the routing and leaves everything about it otherwise alone.
fn seeded(net: &Network, args: &[String]) -> Services {
    let mut services = Services::for_network(net);
    let number = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .and_then(|raw| raw.parse::<f64>().ok())
    };
    if let Some(minutes) = number("--headway") {
        for at in 0..services.categories().len() {
            services.set_headway(at, minutes);
        }
    }
    if let Some(seconds) = number("--walk") {
        services.set_walk(seconds);
    }
    // Matched by name the way --line is, since the categories are named after
    // the drawing and nobody should have to count them to switch one off.
    for (i, arg) in args.iter().enumerate() {
        if arg != "--off" {
            continue;
        }
        let Some(want) = args.get(i + 1) else {
            eprintln!("--off needs the name of a category");
            continue;
        };
        let needle = want.to_lowercase();
        let found = (0..services.categories().len())
            .find(|at| services.categories()[*at].name.to_lowercase().contains(&needle));
        match found {
            Some(at) => services.set_allowed(at, false),
            None => eprintln!("no category matching {want:?}; nothing switched off"),
        }
    }
    services
}

/// How often a service runs, said as the route display says it.
fn every(minutes: f64) -> String {
    let seconds = (minutes.max(0.0) * 60.0).round() as u64;
    match seconds {
        60 => "every minute".to_string(),
        0..=119 => format!("every {seconds} seconds"),
        s if s % 60 == 0 => format!("every {} min", s / 60),
        s => format!("every {:.1} min", s as f64 / 60.0),
    }
}

fn summary(net: &Network, path: &str) {
    let vb = net.view_box;
    let routable: Vec<_> = net.routable_lines().collect();
    let track: f64 = net.edges.iter().map(|e| e.length).sum();
    let components = net.components();
    let unreachable = net.unreachable_stations();
    let named = net.stations.iter().filter(|s| s.is_searchable()).count();

    println!("{path}");
    println!(
        "  viewBox        {} {} {} {}",
        vb.min_x, vb.min_y, vb.width, vb.height
    );
    println!(
        "  stations       {}  ({named} named, {} unnamed, {} interchanges)",
        net.stations.len(),
        net.stations.len() - named,
        net.stations.iter().filter(|s| s.marker == Marker::Interchange).count(),
    );
    println!(
        "  lines          {}  ({} routable, {} excluded)",
        net.lines.len(),
        routable.len(),
        net.lines.len() - routable.len(),
    );
    println!("  segments       {}  ({track:.1} grid units of track)", net.edges.len());
    println!(
        "  routable graph {} piece(s), largest {} stations; {} station(s) unreachable",
        components.len(),
        components.first().map(Vec::len).unwrap_or(0),
        unreachable.len(),
    );
    if !unreachable.is_empty() {
        let named_out = unreachable.iter().filter(|id| net.station(**id).is_searchable()).count();
        println!("                 of those unreachable, {named_out} are named");
    }
    let scale = Scale::for_network(net);
    println!(
        "  scale          centre ({}, {}), profile out to {} units",
        scale.centre.0, scale.centre.1, scale.reach(),
    );

    // What the service panel opens with, since it is seeded from the drawing.
    let services = Services::for_network(net);
    let sorted: Vec<String> = services
        .categories()
        .iter()
        .enumerate()
        .map(|(at, c)| {
            format!("{} {} ({:.0} km/h)", c.name, services.lines_in(at).len(), c.top_speed)
        })
        .collect();
    println!("  services       {}", sorted.join(", "));
}

fn lines(net: &Network, only: Option<&str>) {
    println!();
    println!(
        "  {:<26} {:<7} {:<13} {:>5} {:>6} {:>8}  routable",
        "line", "mode", "shape", "stops", "segs", "length"
    );
    for line in &net.lines {
        if only.is_some_and(|want| !line.name.to_lowercase().contains(&want.to_lowercase())) {
            continue;
        }
        println!(
            "  {:<26} {:<7} {:<13} {:>5} {:>6} {:>8.1}  {}",
            truncate(&line.name, 26),
            match line.mode {
                headway_core::Mode::Metro => "metro",
                headway_core::Mode::Tram => "tram",
                headway_core::Mode::CommuterRail => "rail",
            },
            match line.shape {
                Shape::Path => "path",
                Shape::Cycle => "cycle",
                Shape::Branched => "BRANCHED",
                Shape::Disconnected => "DISCONNECTED",
            },
            line.stops.len(),
            line.edges.len(),
            line.length(net),
            if line.routable { "yes" } else { "no" },
        );
    }

    // The stop order itself, when a single line was asked for.
    if let Some(want) = only {
        for line in net.lines.iter().filter(|l| l.name.to_lowercase().contains(&want.to_lowercase())) {
            println!();
            println!("  {} — {} stops in order:", line.name, line.stops.len());
            for (i, stop) in line.stops.iter().enumerate() {
                let s = net.station(*stop);
                let (x, y) = s.cell.centre();
                println!(
                    "    {:>3}. {:<40} {:<12} ({x}, {y}){}",
                    i + 1,
                    truncate(&net.label(*stop), 40),
                    match s.marker {
                        Marker::Interchange => "interchange".to_string(),
                        Marker::Tick { ticks } => format!("{ticks} tick"),
                    },
                    if s.icons.is_empty() { String::new() } else { format!(" {:?}", s.icons) },
                );
            }
        }
    }
}

fn stations(net: &Network) {
    println!();
    let mut ids: Vec<StationId> = net.stations.iter().map(|s| s.id).collect();
    ids.sort_by_key(|id| std::cmp::Reverse(net.station(*id).edges.len()));
    println!("  busiest stations:");
    for id in ids.iter().take(12) {
        let s = net.station(*id);
        let mut names: Vec<&str> = s
            .edges
            .iter()
            .map(|e| net.line(net.edge(*e).line).name.as_str())
            .collect();
        names.sort_unstable();
        names.dedup();
        println!(
            "    {:>2} lines  {:<38} {}",
            names.len(),
            truncate(&net.label(*id), 38),
            names.join(", "),
        );
    }
}

fn warnings(net: &Network) {
    println!();
    if net.warnings.is_empty() {
        println!("  no warnings");
        return;
    }
    println!("  {} warning(s):", net.warnings.len());
    for w in &net.warnings {
        println!("    - {w}");
    }
}

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(width - 1).collect::<String>())
    }
}

/// Resolves a station the way the web app's search box does, so the two can
/// never drift apart.
fn find(net: &Network, needle: &str) -> Result<StationId, String> {
    if let Some(exact) = net.station_named(needle) {
        return Ok(exact);
    }
    let hits = net.search(needle, 8);
    match hits.len() {
        0 => Err(format!("no station matching {needle:?}")),
        _ => Ok(hits[0]),
    }
}

/// A duration as a timetable prints it.
fn clock(seconds: f64) -> String {
    let whole = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

fn route(
    timing: &Timing,
    scale: &Scale,
    services: &Services,
    from: &str,
    to: &str,
    paint: bool,
) -> ExitCode {
    let net = timing.net();
    let (from, to) = match (find(net, from), find(net, to)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!("{}  ->  {}", net.label(from), net.label(to));
    let off: Vec<&str> = services
        .categories()
        .iter()
        .enumerate()
        .filter(|(at, _)| !services.allows(*at))
        .map(|(_, c)| c.name.as_str())
        .collect();
    if !off.is_empty() {
        println!("  not routing over {}", off.join(", "));
    }
    match options(timing, from, to) {
        Err(why) => {
            println!("  {why}");
            ExitCode::FAILURE
        }
        Ok(picks) => {
            println!(
                "  measured from ({}, {}) at {:.2} km/unit in the centre, {:.2} at the edge",
                scale.centre.0,
                scale.centre.1,
                scale.km_per_unit(0.0),
                scale.km_per_unit(scale.reach()),
            );
            println!(
                "  {} s stood at each stop; a change is {} across the platform \
                 plus half the headway of whatever is boarded",
                timing.dwell,
                clock(timing.walk),
            );
            println!(
                "  {} way{} worth offering:",
                picks.len(),
                if picks.len() == 1 { "" } else { "s" },
            );
            for (n, pick) in picks.iter().enumerate() {
                println!();
                option(timing, scale, services, n + 1, pick);
            }
            let journey = &picks[0].journey;
            // Only the quickest is painted: the browser draws whichever card
            // is picked, and one layer is what there is to check here.
            if paint {
                // Exactly what the web app lays over the map for the option it
                // is showing, which is the quickest until another is picked.
                let sheet = fade(net);
                let drawn = trace(net, journey);
                println!();
                println!("  {} colour(s) fade:", sheet.lines().count());
                print!("{sheet}");
                println!();
                println!("  route layer, {} bytes:", drawn.len());
                // Whole, not truncated: this output exists to be checked
                // against the map's own markup.
                for piece in drawn.split_inclusive("/>") {
                    println!("    {piece}");
                }
                // The debug layer, as the panel's toggle draws it: one
                // distance per stretch of track between two stations.
                let mut hops = Vec::new();
                let mut texts = Vec::new();
                for leg in &journey.legs {
                    for hop in leg.segments(net) {
                        texts.push(format!("{:.1} km", scale.length(&hop)));
                        hops.push(hop);
                    }
                }
                println!();
                println!("  {} section tag(s):", hops.len());
                for piece in tag(&hops, &texts).split_inclusive("</text>") {
                    println!("    {piece}");
                }
            }
            ExitCode::SUCCESS
        }
    }
}

/// One of the journeys on offer, under what makes it worth offering.
fn option(timing: &Timing, scale: &Scale, services: &Services, n: usize, pick: &Choice) {
    let net = timing.net();
    let journey = &pick.journey;
    println!(
        "  [{n}] {:<22}{:>8}  {} stations, {} change(s), {:.1} km",
        pick.merit.label(net),
        clock(journey.seconds),
        journey.stations(),
        journey.transfers(),
        scale.journey_km(net, journey),
    );
    for (i, leg) in journey.legs.iter().enumerate() {
        println!(
            "      {:<28}{:>3} station{}{:>9.1} km{:>8}  {}",
            truncate(&net.line(leg.line).name, 27),
            leg.stations(),
            if leg.stations() == 1 { "" } else { "s" },
            scale.leg_km(net, leg),
            clock(timing.leg(leg)),
            every(services.headway_of(leg.line)),
        );
        if let Some(next) = journey.legs.get(i + 1) {
            println!(
                "      transfer at {:<24}{} walk + {} wait{:>8}",
                truncate(&net.label(leg.alight()), 23),
                clock(timing.walk),
                clock(timing.wait(next.line)),
                clock(timing.boarding(next.line)),
            );
        }
    }
}
