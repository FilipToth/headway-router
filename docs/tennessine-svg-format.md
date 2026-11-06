# Tennessine Metro Designer SVG — structure notes

Reverse-engineered from `muvit-amandleng.svg` (one map, 30 lines, 452 stations).
Every claim below was checked against the file; the counts are from that map.

## 1. File shape

Single line of XML. `<!--Created for free with Tennessine's Metro Designer…-->`, an
SVG 1.1 doctype, then `<svg class="metro-map" viewBox="-95,-118,168,184">` with six
children in paint order:

| # | element | contents | role |
|---|---------|----------|------|
| 0 | `g` (stroke-width 0.3) | 18 `path` + 9 `text` | background areas (pixel-art blobs, `hsl()` fills) and district codes (`FW1`, `NVY`, `KGJ`, …). Decorative. |
| 1 | `g` | 38 `path` + 31 `text` | **the key** — "Key to lines". Only place line *names* exist. |
| 2 | `g` (fill none, linecap square) | *empty* | an unused optional layer. Proof that layer indices are not stable. |
| 3 | `g` | 712 `path` | **track segments** (620 core + 92 decorative overlays). |
| 4 | `g` | 916 children | **stations** — 452 × (marker(s) + label `g`). |
| 5 | `defs` | `xhtml:style` | embedded PT Sans + a `label-icons` icon font. |

Do not address layers by index. Classify by content (see §6, gotcha 3).

## 2. The grid

All geometry sits on a 1-unit grid whose **cell centres are at half-integers**.
Snap a point to its cell with `cell(v) = round(v - 0.5) + 0.5`.

Path steps are axis-aligned or exactly 45° diagonal (`(±1,0)`, `(0,±1)`, `(±1,±1)`).
Lines sharing a corridor are offset perpendicular to the direction of travel by
multiples of **0.095** (and `0.095/√2 ≈ 0.067` on diagonals), so a drawn point is
`cell ± k·0.095`, never exactly the cell centre. Snapping removes the offset.

There is **no scale bar and no real-world distance anywhere in the file.** Grid
units are the only metric available.

## 3. Layer 1 — the key

A white rect, the text `Key to lines`, then per line, in order:

1. a 1-unit sample stroke `M<x> <y>l1 0` with `stroke` = the line colour, `stroke-width` 0.2;
2. optionally one or two *overlay* strokes with the same `d` (see §4);
3. a `text` holding the line name.

So a line's visual identity is the tuple **(stroke colour, overlay styles)**, and the
key is the colour→name dictionary. 30 lines in this map, 28 distinct colours.

## 4. Layer 3 — track segments

712 `path`s, all `fill="none"`, all made only of `M`/`L` — pure polylines.
Two kinds, distinguished by `stroke-width`:

* **core** (`stroke-width="0.2"`, line colour) — 620 of them.
* **overlay** (92) — drawn on top of a core path with a *byte-identical* `d`:
  * `stroke="#fff" stroke-width="0.06"` → tram (double-line look), 62
  * `stroke="white" stroke-width="0.1" stroke-dasharray="0.3 0.2"` → commuter rail, 30

Match overlays to cores by their `d` string.

**Each core path is exactly one inter-station segment of exactly one line.** Verified:
all 620 cores have both endpoints landing on a station cell, and only 2 of the 3509
interior vertices land on a station cell — both genuine pass-throughs (Line 1 and
Line 2 run straight through a tram-only stop at `(-27.5, -0.5)`).

> Interior vertices are corner/offset bookkeeping, **not** stops.

Segment length = polyline length in grid units (median 3.0, min 1.0, max 81.1).

## 5. Layer 4 — stations

A flat run of `marker(s)` followed by one `g` holding the label, 452 times.
Each station occupies a unique grid cell, so the cell is a usable primary key.

| marker | count | meaning | network degree |
|--------|-------|---------|----------------|
| `ellipse rx=ry=0.35 fill=white stroke=black` at the cell centre | 98 | interchange (2+ lines) | 3–13 |
| one tick, length 0.45, offset to one side of the track | 304 | ordinary stop on one line | 2 (4 at a branch junction) |
| one tick, length 0.7, centred on the cell | 38 | **terminus** | 1 |
| two ticks, length 0.33 | 12 | stop shared by exactly two lines | 4 |

Ticks are strokes perpendicular to the track, coloured with the **line's own colour** —
which tells you which line stops there without any geometry.

The label `g` holds 0, 1 or 2 `text` children: Tennessine pre-wraps long names, so
join the children with a space (`Directorate` + `for Mobility` → `Directorate for Mobility`).
52 stations have an empty label `g` (unnamed).

Names may contain private-use characters from the embedded `label-icons` font:

| cp | icon | | cp | icon |
|----|------|-|----|------|
| U+E800 | airport | | U+E804 | place-of-interest |
| U+E801 | bus-station | | U+E805 | cable-car |
| U+E802 | ferry | | U+E806 | train-station |
| U+E803 | spaceport | | U+E807 | tram-station |

Strip them from the display name and keep them as an icon/facility enum. Two stations
in this map are labelled *only* with the tram icon.

## 6. Parse recipe (validated on this map)

1. **Nodes** ← layer 4. Position = centroid of the marker(s), snapped to its cell.
   → 452 nodes, all in distinct cells.
2. **Edges** ← the 620 core paths of layer 3. Endpoints snapped to cells; both are
   guaranteed to be station nodes. Keep the full polyline for drawing and for length.
3. **Line attribution** ← `(stroke, overlay styles)` looked up in the key.
   Uniquely resolves 572/620 edges here; the rest is gotcha 1.
4. **Graph** → one connected component, 452 nodes, 620 edges. Walking each line's
   subgraph from a terminus yields its stop sequence: 24 simple paths, 4 cycles
   (Line 12 "Ring 1", Line 15 "Ring 2", AirTrain A, AirTrain B), plus two cases where
   the drawn subgraph is not a bare path or cycle — see gotcha 8.
5. Line colour + overlay also give the mode (plain = metro, white overlay = tram,
   dashed overlay = commuter rail) — a reasonable default for speed/frequency.

## 7. Gotchas to design around

1. **Colour is not a line id.** `hsl(0,100%,60%)` is both *Line 15 (Ring 2)* and
   *Commuter D* — separable, because only Commuter D carries the dashed overlay.
   But *Forward Tram* and *Forward Tram 2* share colour `hsl(87,100%,50%)` **and**
   the same white overlay: nothing in the file distinguishes them. Their 48 segments
   form one connected branching subgraph (4 termini, one degree-4 and two degree-3
   junctions). Needs a heuristic (trace straightest continuation from each terminus)
   or a manual override in the UI.
2. **The key is optional in Tennessine.** Exported without it, there are no line
   names at all; fall back to synthetic ids per (colour, overlay) plus topology.
3. **Layer indices shift** when optional layers are absent (layer 2 is already empty
   here). Detect layers by content: the key layer contains a `text` reading
   `Key to lines`; the station layer contains `ellipse rx=0.35`; the track layer is
   the large group of `fill="none"` `M`/`L` paths.
4. Don't hard-code the half-integer grid phase — derive it from the modal fractional
   part of the coordinates.
5. **No scale.** The user must supply a grid-units→km factor before any journey time,
   acceleration or dwell model means anything.
6. 52 stations carry no label and 2 are labelled only with an icon glyph. They are
   real stations and stay in the graph as interchange/through nodes; they are simply
   absent from the search index (not queryable as an origin or destination). Identify
   every station by its cell, never by name.
7. Up to 3 parallel edges between the same pair of stations (different lines) → the
   routing graph is a **multigraph** keyed by `(line, a, b)`.
8. **Lines never branch** (Tennessine draws a line as one sequence of cells), so a
   line's drawn subgraph must be coverable by a single walk: within one line's
   subgraph a degree-1 node is a terminus **or a reversal point**, and a degree-3/4
   node is a self-crossing or two same-styled lines meeting — never a junction.
   Branching is **not supported yet**; see §8.
9. A line may pass through a station cell without stopping. Only path *endpoints*
   create a stop.

## 8. Validation and degraded mode

Branching is out of scope for now, but a real map will contain it (author error, or a
tram network, which is branched by nature). The parser must therefore validate each
line independently and degrade per line rather than failing the map:

* Build each line's subgraph and classify it: **path**, **cycle**, or **branched**
  (any node of degree ≥ 3 in that line's own subgraph).
* A branched line raises a **warning naming the line and the junction station(s)**,
  stating that branching is not supported yet.
* A branched line is dropped from the routing graph. **Every other line stays
  usable** — the map must still route.
* Stations left with no usable edge become unroutable and should be reported too, so
  the user can see the cost of the exclusion.

Measured on `muvit-amandleng.svg`:

| dropped | routable network | stations left stranded |
|---------|------------------|------------------------|
| nothing | 1 component, 452 stations | 0 |
| tram group | 1 component, 412 stations | 40 |
| tram group + Line 17 | 1 component, 391 stations | 61 |

All 40 stations stranded by excluding the tram group are **unnamed**, so in this map
the tram exclusion costs nothing that was queryable in the first place.

The two branched cases in this map:

* **Line 17** — a one-cell pendant track from `National Development Commission`
  (-13.5,-2.5) north to `Forward Offices A` (-13.5,-3.5), making NDC degree 3.
  **Confirmed by the author as a mistake in the map**, not a Tennessine quirk: no
  special parser rule, just the warning above. With it removed upstream Line 17 is a
  clean Saburtalo ↔ Xiguan path.
* **Forward Tram / Forward Tram 2** — the author's intent is *one* tram line, and
  trams are heavily branched, so it stays branched even once the naming is sorted out.
  Excluded with a warning for now. (It is also the one style collision in the file:
  the two legend entries share colour **and** overlay, so the 48 segments could not be
  attributed per-line even without the branching.)
