//! The subset of SVG path syntax Tennessine emits.
//!
//! Every path in a Tennessine export is a polyline: `M`/`L` for track and
//! station ticks, with `l` and `z` showing up in the legend box and the
//! pixel-art background. Curves never appear, and if one ever does we want to
//! hear about it rather than silently approximate it.

use core::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// A command we do not implement, such as a curve.
    Unsupported(char),
    /// Coordinates appeared before any command.
    NoCommand,
    /// A command was left without the arguments it needs.
    Truncated(char),
    NotANumber(String),
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(c) => write!(f, "unsupported path command '{c}'"),
            Self::NoCommand => write!(f, "path data starts with a coordinate, not a command"),
            Self::Truncated(c) => write!(f, "command '{c}' is missing arguments"),
            Self::NotANumber(s) => write!(f, "cannot read '{s}' as a number"),
        }
    }
}

impl std::error::Error for PathError {}

/// Parses a `d` attribute into its subpaths, each a list of points.
///
/// A closed subpath (`z`) repeats its first point as its last.
pub fn parse_path(d: &str) -> Result<Vec<Vec<(f64, f64)>>, PathError> {
    let b = d.as_bytes();
    let mut i = 0;
    let mut subpaths: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let mut pos = (0.0, 0.0);
    let mut sub_start = (0.0, 0.0);

    skip_separators(b, &mut i);
    while i < b.len() {
        let cmd = b[i];
        if !cmd.is_ascii_alphabetic() {
            return Err(PathError::NoCommand);
        }
        i += 1;

        if cmd == b'z' || cmd == b'Z' {
            if !cur.is_empty() {
                cur.push(sub_start);
                pos = sub_start;
            }
            skip_separators(b, &mut i);
            continue;
        }
        if !matches!(cmd, b'M' | b'm' | b'L' | b'l' | b'H' | b'h' | b'V' | b'v') {
            return Err(PathError::Unsupported(cmd as char));
        }

        // Each command takes one or more argument groups; a repeated group
        // after `M`/`m` is an implicit line, per the SVG spec.
        let mut group = 0;
        loop {
            skip_separators(b, &mut i);
            if i >= b.len() || b[i].is_ascii_alphabetic() {
                if group == 0 {
                    return Err(PathError::Truncated(cmd as char));
                }
                break;
            }
            let relative = cmd.is_ascii_lowercase();
            match cmd {
                b'H' | b'h' => {
                    let x = read_number(b, &mut i)?;
                    pos.0 = if relative { pos.0 + x } else { x };
                }
                b'V' | b'v' => {
                    let y = read_number(b, &mut i)?;
                    pos.1 = if relative { pos.1 + y } else { y };
                }
                _ => {
                    let x = read_number(b, &mut i)?;
                    skip_separators(b, &mut i);
                    if i >= b.len() || b[i].is_ascii_alphabetic() {
                        return Err(PathError::Truncated(cmd as char));
                    }
                    let y = read_number(b, &mut i)?;
                    pos = if relative { (pos.0 + x, pos.1 + y) } else { (x, y) };
                }
            }

            let starts_subpath = (cmd == b'M' || cmd == b'm') && group == 0;
            if starts_subpath {
                if !cur.is_empty() {
                    subpaths.push(core::mem::take(&mut cur));
                }
                sub_start = pos;
            }
            cur.push(pos);
            group += 1;
        }
    }

    if !cur.is_empty() {
        subpaths.push(cur);
    }
    Ok(subpaths)
}

/// The length of a polyline, in the units it is expressed in.
pub fn polyline_length(points: &[(f64, f64)]) -> f64 {
    points
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt())
        .sum()
}

fn skip_separators(b: &[u8], i: &mut usize) {
    while *i < b.len() && (b[*i].is_ascii_whitespace() || b[*i] == b',') {
        *i += 1;
    }
}

fn read_number(b: &[u8], i: &mut usize) -> Result<f64, PathError> {
    let start = *i;
    if *i < b.len() && (b[*i] == b'+' || b[*i] == b'-') {
        *i += 1;
    }
    while *i < b.len() && b[*i].is_ascii_digit() {
        *i += 1;
    }
    if *i < b.len() && b[*i] == b'.' {
        *i += 1;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
    }
    if *i < b.len() && (b[*i] == b'e' || b[*i] == b'E') {
        let before_exponent = *i;
        *i += 1;
        if *i < b.len() && (b[*i] == b'+' || b[*i] == b'-') {
            *i += 1;
        }
        if *i < b.len() && b[*i].is_ascii_digit() {
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
            }
        } else {
            *i = before_exponent;
        }
    }
    let text = core::str::from_utf8(&b[start..*i]).unwrap_or("");
    text.parse().map_err(|_| PathError::NotANumber(text.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_track_segment() {
        // Line 1 between Forward Commission and Directorate for Mobility
        let d = "M-19.5 -2.69L-20.5 -2.69L-21.5 -2.69";
        assert_eq!(
            parse_path(d).unwrap(),
            vec![vec![(-19.5, -2.69), (-20.5, -2.69), (-21.5, -2.69)]]
        );
    }

    #[test]
    fn reads_a_relative_station_tick() {
        // a 0.45-long tick, as the station layer writes them
        let got = parse_path("M-25.595 7.5l0.44999999999999996 0").unwrap();
        assert_eq!(got.len(), 1);
        assert!((polyline_length(&got[0]) - 0.45).abs() < 1e-12);
    }

    #[test]
    fn reads_scientific_notation() {
        // Tennessine writes near-zero deltas like this
        let got = parse_path("M-21.5 -0.595l-8.266365894244633e-17 0.44999999999999996").unwrap();
        assert_eq!(got[0].len(), 2);
        assert!((got[0][1].1 - -0.145).abs() < 1e-12);
    }

    #[test]
    fn splits_subpaths_and_closes_them() {
        // the legend box, and the multi-subpath background blobs
        let box_path = parse_path("M-94 -116.5l5 0l0 23.5l-5 0z").unwrap();
        assert_eq!(box_path.len(), 1);
        assert_eq!(box_path[0].first(), box_path[0].last(), "z returns to the start");

        let blob = parse_path("M0 0l1 0l0 1M5 5l1 0l0 1").unwrap();
        assert_eq!(blob.len(), 2);
        assert_eq!(blob[1][0], (5.0, 5.0));
    }

    #[test]
    fn treats_repeated_moveto_groups_as_lines() {
        let got = parse_path("M1 1 2 2 3 3").unwrap();
        assert_eq!(got, vec![vec![(1.0, 1.0), (2.0, 2.0), (3.0, 3.0)]]);
    }

    #[test]
    fn handles_horizontal_and_vertical_shorthands() {
        let got = parse_path("M0 0H5V5h-1v-1").unwrap();
        assert_eq!(got, vec![vec![(0.0, 0.0), (5.0, 0.0), (5.0, 5.0), (4.0, 5.0), (4.0, 4.0)]]);
    }

    #[test]
    fn rejects_what_it_will_not_guess_at() {
        assert_eq!(parse_path("M0 0C1 1 2 2 3 3"), Err(PathError::Unsupported('C')));
        assert_eq!(parse_path("1 2 3 4"), Err(PathError::NoCommand));
        assert_eq!(parse_path("M0 0L5"), Err(PathError::Truncated('L')));
        assert_eq!(parse_path("M"), Err(PathError::Truncated('M')));
    }

    #[test]
    fn measures_a_diagonal_run() {
        let pts = parse_path("M0 0L1 1L2 2").unwrap();
        assert!((polyline_length(&pts[0]) - 2.0 * 2f64.sqrt()).abs() < 1e-12);
    }
}
