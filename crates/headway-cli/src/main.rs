//! Drives the parser without a browser, which is how the parser gets checked.
//!
//!     headway <map.svg> [--lines] [--stations] [--warnings] [--line <name>]
//!                       [--route <from> <to>] [--find <query>] [--css]

use headway_core::network::{Marker, Shape};
use headway_core::{fastest, parse, wash_except, Network, StationId};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!(
            "usage: headway <map.svg> [--lines] [--stations] [--warnings] [--line <name>] \
             [--route <from> <to>] [--find <query>] [--css]"
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
        return route(&net, from, to, want("--css"));
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

fn route(net: &Network, from: &str, to: &str, css: bool) -> ExitCode {
    let (from, to) = match (find(net, from), find(net, to)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    println!("{}  ->  {}", net.label(from), net.label(to));
    match fastest(net, from, to) {
        Err(why) => {
            println!("  {why}");
            ExitCode::FAILURE
        }
        Ok(journey) => {
            println!(
                "  {} stations, {} transfer(s), {:.1} units of track, cost {:.1}",
                journey.stations(),
                journey.transfers(),
                journey.distance(net),
                journey.cost,
            );
            println!();
            for (i, leg) in journey.legs.iter().enumerate() {
                println!(
                    "    {:<30}{:>3} station{}",
                    net.line(leg.line).name,
                    leg.stations(),
                    if leg.stations() == 1 { "" } else { "s" },
                );
                if i + 1 < journey.legs.len() {
                    println!("    transfer at {}", net.label(leg.alight()));
                }
            }
            if css {
                // The stylesheet the web app lays over the map for this journey.
                let ridden: Vec<_> = journey.legs.iter().map(|l| l.line).collect();
                let sheet = wash_except(net, &ridden);
                println!();
                println!("  {} line(s) fade:", sheet.lines().count());
                print!("{sheet}");
            }
            ExitCode::SUCCESS
        }
    }
}
