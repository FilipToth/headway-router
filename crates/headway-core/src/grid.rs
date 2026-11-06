//! Tennessine lays a map out on a 1-unit grid whose cell centres sit at
//! half-integers. Drawn track is nudged off that centre, perpendicular to the
//! direction of travel, by multiples of 0.095 so that lines sharing a corridor
//! run as parallel tracks. Snapping to the cell removes that offset, which is
//! what lets a drawn endpoint be matched to the station standing there.

/// A grid cell, identified by the doubled coordinates of its centre so that it
/// is hashable and exact: centre `(-13.5, -2.5)` is `Cell { x2: -27, y2: -5 }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cell {
    pub x2: i32,
    pub y2: i32,
}

impl Cell {
    /// Snaps a drawn point to the cell containing it.
    pub fn snap(x: f64, y: f64) -> Self {
        Self { x2: snap_axis(x), y2: snap_axis(y) }
    }

    /// The centre of this cell, in SVG user units.
    pub fn centre(self) -> (f64, f64) {
        (self.x2 as f64 / 2.0, self.y2 as f64 / 2.0)
    }
}

fn snap_axis(v: f64) -> i32 {
    // nearest half-integer, doubled: round(v - 0.5) + 0.5, times two
    2 * (v - 0.5).round() as i32 + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centre_round_trips() {
        for (x, y) in [(-13.5, -2.5), (0.5, 0.5), (45.5, -56.5), (-0.5, 13.5)] {
            assert_eq!(Cell::snap(x, y).centre(), (x, y), "{x},{y}");
        }
    }

    #[test]
    fn strips_the_track_offset() {
        let centre = Cell::snap(-13.5, -2.5);
        // the three lines sharing the Forward Offices A <-> NDC corridor sit in
        // slots -0.19 / 0 / +0.19, and all three must land on the same cell
        for off in [-0.19, 0.0, 0.19, -0.285, 0.285] {
            assert_eq!(Cell::snap(-13.5, -2.5 + off), centre, "offset {off}");
            assert_eq!(Cell::snap(-13.5 + off, -2.5), centre, "offset {off}");
        }
        // a diagonal track is offset by 0.095/sqrt(2) on each axis
        let d = 0.095 / 2f64.sqrt();
        assert_eq!(Cell::snap(-13.5 + d, -2.5 - d), centre);
    }

    #[test]
    fn neighbouring_cells_stay_distinct() {
        let a = Cell::snap(-13.5, -3.5); // Forward Offices A
        let b = Cell::snap(-13.5, -2.5); // National Development Commission
        assert_ne!(a, b);
        assert_eq!(b.y2 - a.y2, 2);
    }

    #[test]
    fn works_across_the_origin() {
        assert_eq!(Cell::snap(-0.5, 0.5).centre(), (-0.5, 0.5));
        assert_eq!(Cell::snap(-0.4, 0.6).centre(), (-0.5, 0.5));
        assert_eq!(Cell::snap(-0.6, 0.4).centre(), (-0.5, 0.5));
    }
}
