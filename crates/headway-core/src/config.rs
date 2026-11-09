//! A map and everything decided about it, as one file.
//!
//! None of the work a user puts into a map is in the map. Tennessine's SVG
//! says nothing about how far a unit is, how fast a metro runs, or how often
//! it comes; all of that is theirs, and until now it lived for exactly as long
//! as the browser tab. So a saved file carries the map *and* the settings
//! together: open it and you are back where you left off, and send it to
//! somebody else and they see what you saw.
//!
//! The map travels inside the file rather than beside it on purpose. A profile
//! is measured in that map's grid units from a point in that map's
//! coordinates, and a category holds lines by their place in that map's
//! legend. Settings and map are one thing, and splitting them would only
//! invite the two halves apart.
//!
//! # The format
//!
//! Lines of `key value`, then a line reading `svg`, then the map verbatim to
//! the end of the file:
//!
//! ```text
//! headway 1
//! name "muvit-amandleng.svg"
//! centre -12.5 -15.5
//! axis 150
//! walk 60
//! profile 0:0 13.6:4.1 27.2:9.6 40.8:17.9
//! category "Metro" 80 1 1.2 5 on
//! category "Commuter rail" 140 0.7 0.9 20 off
//! line 0 0 - "Line 1"
//! line 1 1 7.5 "Commuter D"
//! svg
//! <svg xmlns="http://www.w3.org/2000/svg" ...
//! ```
//!
//! Text, because a map is text and a setting is a number: it diffs, it can be
//! read without this program, and the SVG needs no escaping when it is last.
//! Numbers are written with Rust's shortest round-tripping form, so a profile
//! dragged to a particular shape comes back as that shape and not near it.
//!
//! Reading is deliberately forgiving in one direction only. A key it does not
//! know is ignored, so a file from a later version still opens; but every
//! number goes in through [`Services`]'s own setters and [`Scale::from_points`],
//! so a hand-edited file cannot put a speed or a profile into the app that the
//! app would not have let anyone type. What it cannot make sense of, it says
//! so about: [`Config::services`] hands back notes alongside the settings.

use crate::network::{Mode, Network};
use crate::scale::Scale;
use crate::service::Services;
use std::fmt;

/// The format this writes. Read refuses anything higher, because a file from
/// the future may mean something different by a key we do think we know.
pub const VERSION: u32 = 1;

/// What a saved file is called.
pub const EXTENSION: &str = "headway";

/// The line that ends the settings and begins the map.
const MAP_MARKER: &str = "svg";

/// A saved category, as written: the numbers have not been through
/// [`Services`]'s clamps yet.
#[derive(Debug, Clone, PartialEq)]
struct Kind {
    name: String,
    top_speed: f64,
    accel: f64,
    brake: f64,
    headway: f64,
    allowed: bool,
}

/// A saved line: where it sat in the map's legend, what it was called there,
/// which category it was put in, and its own frequency if it had one.
#[derive(Debug, Clone, PartialEq)]
struct Which {
    index: usize,
    name: String,
    category: usize,
    headway: Option<f64>,
}

/// Everything a saved file holds.
///
/// The settings stay as written until [`Config::services`] is handed the map
/// they belong to, because a line's category is held by its place in that map
/// and there is nothing to apply it to until the map is parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// What the map was called when it was opened.
    pub name: String,
    /// The map itself, byte for byte.
    pub svg: String,
    pub scale: Scale,
    /// The kilometre axis the profile was last framed against — a view of the
    /// profile rather than part of it, so a file may leave it out.
    pub axis: Option<f64>,
    walk: f64,
    categories: Vec<Kind>,
    lines: Vec<Which>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// Not one of ours at all.
    NotOurs,
    /// Written by a later version than this one knows.
    Newer(u32),
    /// No `svg` line, so there is no map in it.
    NoMap,
    /// The map is there but empty.
    EmptyMap,
    /// Not one category, so there is nothing to put a line in.
    NoCategories,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOurs => write!(f, "this is not a Headway file"),
            Self::Newer(v) => write!(
                f,
                "this file was saved by a newer Headway (version {v}, this one reads {VERSION})",
            ),
            Self::NoMap => write!(f, "the file has no map in it"),
            Self::EmptyMap => write!(f, "the map in the file is empty"),
            Self::NoCategories => write!(f, "the file lists no service categories"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Everything about a map as it stands, ready to be written.
    pub fn of(
        name: &str,
        svg: &str,
        scale: &Scale,
        axis: Option<f64>,
        services: &Services,
        net: &Network,
    ) -> Self {
        let categories = services
            .categories()
            .iter()
            .enumerate()
            .map(|(at, c)| Kind {
                name: c.name.clone(),
                top_speed: c.top_speed,
                accel: c.accel,
                brake: c.brake,
                headway: c.headway,
                allowed: services.allows(at),
            })
            .collect();
        // Every line, not only the ones that were moved: a file that says
        // where everything is can be read by somebody who has never seen the
        // app, and the whole set is a few dozen lines.
        let lines = net
            .lines
            .iter()
            .map(|line| Which {
                index: line.id.0,
                name: line.name.clone(),
                category: services.category_of(line.id),
                headway: services.own_headway(line.id),
            })
            .collect();
        Self {
            name: name.to_string(),
            svg: svg.to_string(),
            scale: scale.clone(),
            axis,
            walk: services.walk(),
            categories,
            lines,
        }
    }

    pub fn write(&self) -> String {
        let mut out = format!("headway {VERSION}\n");
        out.push_str(&format!("name {}\n", quoted(&self.name)));
        out.push_str(&format!("centre {} {}\n", self.scale.centre.0, self.scale.centre.1));
        if let Some(axis) = self.axis {
            out.push_str(&format!("axis {axis}\n"));
        }
        out.push_str(&format!("walk {}\n", self.walk));
        let profile: Vec<String> =
            self.scale.points().iter().map(|(r, km)| format!("{r}:{km}")).collect();
        out.push_str(&format!("profile {}\n", profile.join(" ")));
        for kind in &self.categories {
            out.push_str(&format!(
                "category {} {} {} {} {} {}\n",
                quoted(&kind.name),
                kind.top_speed,
                kind.accel,
                kind.brake,
                kind.headway,
                match kind.allowed {
                    true => "on",
                    false => "off",
                },
            ));
        }
        for which in &self.lines {
            out.push_str(&format!(
                "line {} {} {} {}\n",
                which.index,
                which.category,
                match which.headway {
                    Some(minutes) => minutes.to_string(),
                    None => "-".to_string(),
                },
                quoted(&which.name),
            ));
        }
        out.push_str(MAP_MARKER);
        out.push('\n');
        out.push_str(&self.svg);
        out
    }

    /// Whether a file looks like one of ours, for telling a saved file from a
    /// plain map without trying to read either.
    pub fn is_ours(text: &str) -> bool {
        text.trim_start().starts_with("headway ")
    }

    pub fn read(text: &str) -> Result<Self, ConfigError> {
        if !Self::is_ours(text) {
            return Err(ConfigError::NotOurs);
        }

        let mut cursor = 0usize;
        let mut map_from = None;
        let mut header: Vec<String> = Vec::new();
        for raw in text.split_inclusive('\n') {
            cursor += raw.len();
            let line = raw.trim_end_matches('\n').trim_end_matches('\r');
            if line.trim() == MAP_MARKER {
                map_from = Some(cursor);
                break;
            }
            header.push(line.to_string());
        }
        let svg = &text[map_from.ok_or(ConfigError::NoMap)?..];
        if svg.trim().is_empty() {
            return Err(ConfigError::EmptyMap);
        }

        let mut name = String::new();
        let mut centre = (0.0, 0.0);
        let mut axis = None;
        let mut walk = crate::service::DEFAULT_WALK;
        let mut points: Vec<(f64, f64)> = Vec::new();
        let mut categories: Vec<Kind> = Vec::new();
        let mut lines: Vec<Which> = Vec::new();

        for line in &header {
            let words = words(line);
            let Some(key) = words.first() else {
                continue;
            };
            let rest = &words[1..];
            match key.as_str() {
                "headway" => match rest.first().and_then(|v| v.parse::<u32>().ok()) {
                    // A version we do not know may mean something else by a
                    // key we think we do, so this is where reading stops.
                    Some(v) if v > VERSION => return Err(ConfigError::Newer(v)),
                    Some(_) => {}
                    None => return Err(ConfigError::NotOurs),
                },
                "name" => name = rest.first().cloned().unwrap_or_default(),
                "centre" => {
                    if let (Some(Ok(x)), Some(Ok(y))) =
                        (rest.first().map(|v| v.parse()), rest.get(1).map(|v| v.parse()))
                    {
                        centre = (x, y);
                    }
                }
                "axis" => axis = rest.first().and_then(|v| v.parse::<f64>().ok()),
                "walk" => {
                    if let Some(Ok(seconds)) = rest.first().map(|v| v.parse()) {
                        walk = seconds;
                    }
                }
                "profile" => {
                    points = rest
                        .iter()
                        .filter_map(|pair| {
                            let (r, km) = pair.split_once(':')?;
                            Some((r.parse().ok()?, km.parse().ok()?))
                        })
                        .collect();
                }
                "category" => {
                    if let Some(kind) = kind(rest) {
                        categories.push(kind);
                    }
                }
                "line" => {
                    if let Some(which) = which(rest) {
                        lines.push(which);
                    }
                }
                // Anything else is from a version that knew more than this one.
                _ => {}
            }
        }

        if categories.is_empty() {
            return Err(ConfigError::NoCategories);
        }

        Ok(Self {
            name,
            svg: svg.to_string(),
            scale: Scale::from_points(centre, points),
            axis,
            walk,
            categories,
            lines,
        })
    }

    /// The settings this file describes, over the map it describes them for.
    ///
    /// Everything goes in through [`Services`]'s own setters, so a figure a
    /// hand-edited file asks for that the app would not have allowed is
    /// clamped here exactly as it would have been there. Anything that cannot
    /// be placed at all comes back as a note rather than being applied to the
    /// wrong line in silence.
    pub fn services(&self, net: &Network) -> (Services, Vec<String>) {
        let mut notes = Vec::new();
        let first = &self.categories[0];
        let mut services = Services::fresh(net.lines.len(), &first.name, Mode::Metro);
        settle(&mut services, 0, first);
        for kind in &self.categories[1..] {
            let at = services.add(&kind.name);
            settle(&mut services, at, kind);
        }
        services.set_walk(self.walk);

        for which in &self.lines {
            // Normally the line is exactly where it was saved. When it is not
            // — a map edited since, or a legend read differently — the name is
            // the better handle, and either way it is worth saying so.
            let found = match net.lines.get(which.index) {
                Some(line) if line.name == which.name => Some(line.id),
                _ => {
                    let by_name = net.lines.iter().find(|line| line.name == which.name);
                    match by_name {
                        Some(line) => {
                            notes.push(format!(
                                "{} has moved in this map since it was saved; matched by name",
                                which.name,
                            ));
                            Some(line.id)
                        }
                        None => {
                            notes.push(format!(
                                "the file has settings for {}, which this map has no line called",
                                which.name,
                            ));
                            None
                        }
                    }
                }
            };
            let Some(id) = found else {
                continue;
            };
            match which.category < services.categories().len() {
                true => services.assign(id, which.category),
                false => notes.push(format!(
                    "{} was in a category the file does not describe; left in {}",
                    which.name,
                    services.categories()[0].name,
                )),
            }
            services.set_own_headway(id, which.headway);
        }
        (services, notes)
    }
}

/// Writes a category's saved numbers onto a live one, through the setters that
/// keep them sane.
fn settle(services: &mut Services, at: usize, kind: &Kind) {
    services.rename(at, &kind.name);
    services.set_top_speed(at, kind.top_speed);
    services.set_accel(at, kind.accel);
    services.set_brake(at, kind.brake);
    services.set_headway(at, kind.headway);
    services.set_allowed(at, kind.allowed);
}

fn kind(words: &[String]) -> Option<Kind> {
    Some(Kind {
        name: words.first()?.clone(),
        top_speed: words.get(1)?.parse().ok()?,
        accel: words.get(2)?.parse().ok()?,
        brake: words.get(3)?.parse().ok()?,
        headway: words.get(4)?.parse().ok()?,
        // Missing says on: a file that does not mention the switches is from
        // before there were any, and everything was usable then.
        allowed: words.get(5).map(|v| v != "off").unwrap_or(true),
    })
}

fn which(words: &[String]) -> Option<Which> {
    Some(Which {
        index: words.first()?.parse().ok()?,
        category: words.get(1)?.parse().ok()?,
        headway: words.get(2).and_then(|v| v.parse().ok()),
        name: words.get(3).cloned().unwrap_or_default(),
    })
}

/// A string as the format writes one: in quotes, with the quote, the backslash
/// and the newline spelled out so that a category called `He said "no"` comes
/// back as itself and a name can never break the line it is written on.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// A header line's words, keeping anything in quotes together.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        let mut word = String::new();
        if c == '"' {
            // An unclosed quote takes the rest of the line, which is the least
            // surprising thing to do with a file somebody has edited by hand.
            while let Some(c) = chars.next() {
                match c {
                    '"' => break,
                    '\\' => match chars.next() {
                        Some('n') => word.push('\n'),
                        Some(other) => word.push(other),
                        None => {}
                    },
                    other => word.push(other),
                }
            }
        } else {
            word.push(c);
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                word.push(c);
                chars.next();
            }
        }
        out.push(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{MAX_SPEED, MIN_RATE};
    use crate::{parse, LineId};

    /// Three lines, so that there is something to sort into categories.
    const MAP: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0,0,24,12">
      <g>
        <path d="M0 0l3 0l0 3l-3 0z" fill="white" stroke="rgb(223,223,247)" stroke-width="0.05"/>
        <text>Key to lines</text>
        <path d="M0.5 0.5l1 0" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <text>North</text>
        <path d="M0.5 1l1 0" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <text>East</text>
        <path d="M0.5 1.5l1 0" fill="none" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
        <text>Scenic</text>
      </g>
      <g>
        <path d="M1.5 1.5L1.5 2.5L1.5 3.5" fill="none" stroke="hsl(0,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 3.5L2.5 3.5L3.5 3.5" fill="none" stroke="hsl(210,100%,50%)" stroke-width="0.2"/>
        <path d="M1.5 1.5L2.5 2.5L3.5 3.5" fill="none" stroke="hsl(120,100%,30%)" stroke-width="0.2"/>
      </g>
      <g>
        <ellipse cx="1.5" cy="1.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Pier</text></g>
        <ellipse cx="1.5" cy="3.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Quay</text></g>
        <ellipse cx="3.5" cy="3.5" rx="0.35" ry="0.35" fill="white" stroke="black" stroke-width="0.16"/>
        <g><text>Tide</text></g>
      </g>
    </svg>"##;

    fn net() -> Network {
        parse(MAP).expect("fixture parses")
    }

    /// Settings nothing like the defaults, so that a round trip that quietly
    /// fell back to them could not pass.
    fn fiddled(net: &Network) -> (Scale, Services) {
        let scale = Scale::from_points((-3.5, 12.0), vec![(10.0, 4.0), (30.0, 25.0)]);
        let mut services = Services::for_network(net);
        services.rename(0, "Underground");
        services.set_top_speed(0, 95.0);
        services.set_accel(0, 1.4);
        services.set_brake(0, 1.6);
        services.set_headway(0, 2.5);
        let express = services.add("Express \"fast\" rail");
        services.set_top_speed(express, 180.0);
        services.set_allowed(express, false);
        services.assign(LineId(2), express);
        services.set_own_headway(LineId(1), Some(7.5));
        services.set_walk(135.0);
        (scale, services)
    }

    #[test]
    fn a_saved_file_comes_back_as_what_was_saved() {
        let net = net();
        let (scale, services) = fiddled(&net);
        let written =
            Config::of("muvit.svg", MAP, &scale, Some(150.0), &services, &net).write();

        let read = Config::read(&written).expect("our own file reads");
        assert_eq!(read.name, "muvit.svg");
        assert_eq!(read.axis, Some(150.0));
        assert_eq!(read.scale, scale, "centre and profile both");
        assert_eq!(read.svg, MAP, "the map byte for byte");

        // The map in the file parses to the same map, so the line indices the
        // categories are held by still mean what they meant.
        let again = parse(&read.svg).expect("the saved map parses");
        let (back, notes) = read.services(&again);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(back, services, "every category, every line, and the walk");
    }

    #[test]
    fn a_round_trip_is_stable_the_second_time_too() {
        // Writing what was read has to give the same file back, or every save
        // would drift a little from the last.
        let net = net();
        let (scale, services) = fiddled(&net);
        let once = Config::of("m.svg", MAP, &scale, None, &services, &net).write();
        let read = Config::read(&once).expect("reads");
        let (back, _) = read.services(&parse(&read.svg).expect("parses"));
        let twice = Config::of(&read.name, &read.svg, &read.scale, read.axis, &back, &net).write();
        assert_eq!(once, twice);
        assert!(!once.contains("axis"), "an axis nobody set is not written");
    }

    #[test]
    fn a_name_with_quotes_and_backslashes_survives() {
        let net = net();
        let mut services = Services::for_network(&net);
        services.rename(0, r#"He said "go\now""#);
        let scale = Scale::for_network(&net);
        let written = Config::of("odd \"name\".svg", MAP, &scale, None, &services, &net).write();
        let read = Config::read(&written).expect("reads");
        assert_eq!(read.name, "odd \"name\".svg");
        let (back, notes) = read.services(&net);
        assert_eq!(back.categories()[0].name, r#"He said "go\now""#);
        assert!(notes.is_empty());
        // And a name cannot smuggle in a line of its own.
        let mut sneaky = Services::for_network(&net);
        sneaky.rename(0, "Metro\nsvg\n<svg/>");
        let written = Config::of("m.svg", MAP, &scale, None, &sneaky, &net).write();
        let read = Config::read(&written).expect("reads");
        assert_eq!(read.svg, MAP, "the map is still the map");
        assert_eq!(read.services(&net).0.categories()[0].name, "Metro\nsvg\n<svg/>");
    }

    #[test]
    fn what_is_not_one_of_ours_is_said_so_about() {
        assert_eq!(Config::read(MAP), Err(ConfigError::NotOurs));
        assert_eq!(Config::read(""), Err(ConfigError::NotOurs));
        assert!(!Config::is_ours(MAP));
        assert!(Config::is_ours("headway 1\nsvg\n<svg/>"));

        // A file from a later version is refused rather than half read.
        let newer = format!("headway {}\ncategory \"M\" 80 1 1 5 on\nsvg\n<svg/>", VERSION + 1);
        assert_eq!(Config::read(&newer), Err(ConfigError::Newer(VERSION + 1)));
        assert_eq!(Config::read("headway x\nsvg\n<svg/>"), Err(ConfigError::NotOurs));

        // And a file with no map, or no map left in it, is no use.
        assert_eq!(
            Config::read("headway 1\ncategory \"M\" 80 1 1 5 on\n"),
            Err(ConfigError::NoMap),
        );
        assert_eq!(
            Config::read("headway 1\ncategory \"M\" 80 1 1 5 on\nsvg\n   \n"),
            Err(ConfigError::EmptyMap),
        );
        assert_eq!(Config::read("headway 1\nsvg\n<svg/>"), Err(ConfigError::NoCategories));
    }

    #[test]
    fn a_file_from_a_later_version_still_opens_if_it_only_added_keys() {
        // Unknown keys are ignored, so a file that gained a setting this
        // version has never heard of is not a file this version cannot open.
        let text = "headway 1\n\
                    name \"m.svg\"\n\
                    weather \"sunny\"\n\
                    category \"Metro\" 80 1 1.2 5 on\n\
                    seats 48\n\
                    line 0 0 - \"North\"\n\
                    svg\n<svg/>";
        let read = Config::read(text).expect("reads");
        assert_eq!(read.name, "m.svg");
        assert_eq!(read.categories.len(), 1);
    }

    #[test]
    fn a_hand_edited_file_cannot_ask_for_what_the_app_would_refuse() {
        let net = net();
        let text = "headway 1\n\
                    walk -40\n\
                    profile 10:4 nonsense 20:NaN 30:9\n\
                    category \"Fast\" 9000 0 -2 0 on\n\
                    category \"Odd\" NaN NaN NaN NaN off\n\
                    line 0 1 0.01 \"North\"\n\
                    svg\n<svg/>";
        let read = Config::read(text).expect("reads");
        let (services, notes) = read.services(&net);
        assert!(notes.is_empty(), "{notes:?}");

        // Clamped exactly as typing them in would have been.
        assert_eq!(services.categories()[0].top_speed, MAX_SPEED);
        assert_eq!(services.categories()[0].accel, MIN_RATE);
        assert_eq!(services.categories()[0].brake, MIN_RATE);
        assert_eq!(services.categories()[0].headway, crate::service::MIN_HEADWAY);
        assert_eq!(services.walk(), 0.0);
        assert!(!services.allows(1));
        // A NaN leaves the value that was there rather than poisoning a time.
        assert!(services.categories()[1].top_speed.is_finite());
        assert_eq!(services.headway_of(LineId(0)), crate::service::MIN_HEADWAY);
        // The profile keeps the points that were numbers and drops the rest.
        assert_eq!(read.scale.points(), &[(0.0, 0.0), (10.0, 4.0), (30.0, 9.0)]);
    }

    #[test]
    fn a_line_the_map_no_longer_has_is_reported_not_guessed() {
        let net = net();
        let text = "headway 1\n\
                    category \"Metro\" 80 1 1.2 5 on\n\
                    category \"Tram\" 50 1.2 1.5 8 on\n\
                    line 0 1 - \"North\"\n\
                    line 7 1 - \"Ghost\"\n\
                    line 1 9 - \"East\"\n\
                    svg\n<svg/>";
        let (services, notes) = Config::read(text).expect("reads").services(&net);
        assert_eq!(services.category_of(LineId(0)), 1, "the one that is still there");
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains("Ghost"), "{notes:?}");
        assert!(notes[1].contains("East"), "{notes:?}");
        assert_eq!(services.category_of(LineId(1)), 0, "left where it was");
    }

    #[test]
    fn a_line_that_has_moved_in_the_map_is_matched_by_name() {
        let net = net();
        // Saved when East was first in the legend; it is second in this map.
        let text = "headway 1\n\
                    category \"Metro\" 80 1 1.2 5 on\n\
                    category \"Tram\" 50 1.2 1.5 8 on\n\
                    line 0 1 3 \"East\"\n\
                    svg\n<svg/>";
        let (services, notes) = Config::read(text).expect("reads").services(&net);
        let east = net.lines.iter().find(|l| l.name == "East").expect("East").id;
        assert_eq!(services.category_of(east), 1);
        assert_eq!(services.own_headway(east), Some(3.0));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("moved"), "{notes:?}");
    }

    #[test]
    fn a_file_written_on_windows_reads_the_same() {
        let net = net();
        let (scale, services) = fiddled(&net);
        let written = Config::of("m.svg", MAP, &scale, Some(50.0), &services, &net).write();
        let crlf = written.replace('\n', "\r\n");
        let read = Config::read(&crlf).expect("reads");
        assert_eq!(read.axis, Some(50.0));
        assert_eq!(read.scale, scale);
        let (back, notes) = read.services(&net);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(back, services);
    }

    #[test]
    fn the_words_of_a_line_are_split_the_way_the_format_writes_them() {
        assert_eq!(words("category \"Metro\" 80 1"), ["category", "Metro", "80", "1"]);
        assert_eq!(words(r#"name "a b"  c "#), ["name", "a b", "c"]);
        assert_eq!(words(r#"x "quote \" inside""#), ["x", "quote \" inside"]);
        assert_eq!(words(r#"x "slash \\ inside""#), ["x", "slash \\ inside"]);
        assert_eq!(words(r#"x "two\nlines""#), ["x", "two\nlines"]);
        assert_eq!(words(r#"x "unclosed"#), ["x", "unclosed"]);
        assert_eq!(words(""), Vec::<String>::new());
        assert_eq!(words("   "), Vec::<String>::new());
        // What is written is what is read.
        for odd in ["", "a\"b", "a\\b", "tab\there", "\u{e800} icon", "線 9"] {
            assert_eq!(words(&format!("k {}", quoted(odd))), ["k", odd]);
        }
    }
}
