# Tennessine Metro Designer SVG — structure notes

Reverse-engineered from `muvit-amandleng.svg` (one map, 30 lines, 452 stations),
then confirmed by `headway-core`, which parses it. Every claim below was checked
against the file; the counts are from that map as re-exported on 2026-11-06.

## 1. File shape

Single line of XML. `<!--Created for free with Tennessine's Metro Designer…-->`, an
SVG 1.1 doctype, then `<svg class="metro-map" viewBox="-95,-118,168,184">` with six
children in paint order. **The DOCTYPE must be allowed explicitly** by a strict XML
parser — roxmltree rejects the file outright without `ParsingOptions::allow_dtd`.

| # | element | contents | role |
|---|---------|----------|------|
| 0 | `g` (stroke-width 0.3) | 18 `path` + 9 `text` | background areas (pixel-art blobs, `hsl()` fills) and district codes (`FW1`, `NVY`, `KGJ`, …). Decorative. |
| 1 | `g` | 38 `path` + 31 `text` | **the key** — "Key to lines". Only place line *names* exist. |
| 2 | `g` (fill none, linecap square) | *empty* | an unused optional layer. Proof that layer indices are not stable. |
| 3 | `g` | 711 `path` | **track segments** (619 core + 92 decorative overlays). |
| 4 | `g` | 917 children | **stations** — 453 × (marker(s) + label `g`), over 452 cells. |
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

* **core** (`stroke-width="0.2"`, line colour) — 619 of them.
* **overlay** (92) — drawn on top of a core path with a *byte-identical* `d`:
  * `stroke="#fff" stroke-width="0.06"` → tram (double-line look), 62
  * `stroke="white" stroke-width="0.1" stroke-dasharray="0.3 0.2"` → commuter rail, 30

Match overlays to cores by their `d` string: for a given `d`, the **widest** stroke
is the track and the rest are decoration. That rule beats hard-coding `0.2`, which
is a line-width setting and will differ between maps.

**Each core path is exactly one inter-station segment of exactly one line.** Verified:
every core has both endpoints landing on a station cell, and only 2 of ~3500 interior
vertices land on a station cell — both genuine pass-throughs (Line 1 and Line 2 run
straight through a tram-only stop at `(-27.5, -0.5)`).

> Interior vertices are corner/offset bookkeeping, **not** stops.

Segment length = polyline length in grid units (median 3.0, min 1.0, max 81.1).

## 5. Layer 4 — stations

A flat run of `marker(s)` followed by one `g` holding the label, 453 times for 452
stations. The cell is the primary key, because **Tennessine can emit the same station
twice**: `Fika Pelong` appears at indices 352/353 and again at 625/626 with byte-identical
markup. Merge onto the cell rather than creating a second node, or the copy becomes an
orphan with no track.

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
   → 452 nodes in 452 cells, after merging the duplicate marker pair.
2. **Edges** ← the 619 core paths of layer 3. Endpoints snapped to cells; both are
   guaranteed to be station nodes. Keep the full polyline for drawing and for length.
3. **Line attribution** ← `(stroke, overlay styles)` looked up in the key.
   Resolves every edge but the 48 tram ones; the rest is gotcha 1.
4. **Graph** → one connected component, 452 nodes, 619 edges. Walking each line's
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
6. A station can be **drawn twice**, identically. Key stations by cell and merge.
7. 52 stations carry no label and 2 are labelled only with an icon glyph. They are
   real stations and stay in the graph as interchange/through nodes; they are simply
   absent from the search index (not queryable as an origin or destination). Identify
   every station by its cell, never by name.
8. Up to 3 parallel edges between the same pair of stations (different lines) → the
   routing graph is a **multigraph** keyed by `(line, a, b)`.
9. **Lines never branch** (Tennessine draws a line as one sequence of cells), so a
   line's drawn subgraph must be coverable by a single walk: within one line's
   subgraph a degree-1 node is a terminus **or a reversal point**, and a degree-3/4
   node is a self-crossing or two same-styled lines meeting — never a junction.
   Branching is **not supported yet**; see §8.
10. A line may pass through a station cell without stopping. Only path *endpoints*
    create a stop.
11. **Nothing in the markup names a line.** No `id`, no `class`, no grouping per
    line — a path's only handle is its `stroke` attribute, and lines sharing a
    colour share that handle (§7.1). So re-styling one line in the browser means
    either a CSS attribute selector on the stroke, which is what `highlight::fade`
    does, or rewriting half a megabyte of SVG. Match on the attribute **exactly as
    written**: `rgb(0, 0, 255)` and `rgb(0,0,255)` are the same colour but different
    selectors, which is why `Line` keeps both a normalised `colour` and the file's
    own `strokes`. Usefully, the track carries `fill="none"` and a station's tick
    carries no `fill` at all, while every filled shape is a background area — so
    `path[fill="none"][stroke=C], path[stroke=C]:not([fill])` reaches a line's track
    and its ticks and touches nothing else. Because of that, anything *we* draw in
    a line's colour must be exempted from those rules — hence `highlight::LEG_CLASS`,
    the only `class` attribute anywhere in the document.
12. **Layer order is a z-order we can splice.** The station layer is a direct `<g>`
    child of the root, with the track layers before it and `defs`/fonts after, so
    the byte offset of its opening tag (`Network::station_layer_at`, from
    roxmltree's `Node::range()`) is a clean seam: markup inserted there draws over
    every line but under every marker and label. That is where a highlighted route
    belongs — at a transfer the interchange's own circle still reads on top of it,
    and no station name is ever covered. Nothing else in the file crosses the seam:
    no `fill="none"` path appears after it.
13. **Drawn track is not where the station is.** A line in a shared corridor is
    offset perpendicular from the cell centre by multiples of 0.095 (§2), so a
    segment's endpoint sits *near* its stop, not on it. Markers drawn along a route
    must follow the geometry (`Leg::stop_points`), not the station's cell, or they
    float beside the line. The same offset leaves a ≤0.095 gap where one line hands
    over to another at a transfer; round caps close it.

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

All 40 stations stranded by excluding the tram group are **unnamed**, so in this map
the tram exclusion costs nothing that was queryable in the first place.

Only one branched case remains:

* **Forward Tram / Forward Tram 2** — the author's intent is *one* tram line, and
  trams are heavily branched, so it stays branched however the naming is resolved.
  Excluded with a warning. It is also the one style collision in the file: the two
  legend entries share colour **and** overlay, so its 48 segments could not be
  attributed per-line even without the branching.

**Line 17** used to branch too: a one-cell pendant track from `National Development
Commission` (-13.5,-2.5) north to `Forward Offices A` (-13.5,-3.5). The author
confirmed that as a mistake in the map and removed it; Line 17 now parses as a clean
33-stop Saburtalo ↔ Xiguan path. It is recorded here because it is the shape a
mis-drawn line takes, and because removing a line from a corridor **re-slots the
others**: Line 7 and Commuter C moved from ±0.19 to new offsets when it went.
