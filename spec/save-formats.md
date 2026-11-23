# Circuit documents

Component center coordinates and wire endpoint coordinates must be in the inclusive range `-100000…100000` on each axis. Node IDs must be nonnegative integers no greater than `2^63−1`.

The desktop extension is `.crtm`. The `.crtmi` extension is reserved for
the future web edition. Cirtam 0.1 reads and writes UTF-8 JSON with the
top-level fields `format` (the string `cirtam`), `version`, and `circuit`.
The editor saves version 2 with explicit connection records. Version 1
documents with either `cirtam` or the legacy `cirmat` identifier still
open. Older readers need an update to read version 2.

The circuit contains `components` and `wires` arrays. Each component
contains:

| Field | Meaning |
| --- | --- |
| `kind` | `Resistor`, `Voltage`, `Current`, `Ground`, `Voltmeter`, or `Ammeter` |
| `at` | Center as integer grid coordinates `[x, y]`, with y increasing down |
| `rotation` | Clockwise quarter-turns from 0 to 3 |
| `label` | User label, possibly empty |
| `value` | Finite SI value in ohms, volts, or amperes. Ground and ideal meters require 0. |
| `ratings` | Optional resistor-only object with `power_watts` (positive watts) and `tolerance_percent` (0–100, as a percentage). Either field can be omitted. |
| `actual_ohms` | Optional stored sample for a resistor with tolerance. It must be finite, positive, and within the nominal tolerance range. When absent, the solver uses `value`. |

Older documents with no `ratings` load with both fields unspecified. Loading
valid geometry with invalid electrical values lets the editor show and repair
the affected parts. DC analysis and saving require valid electrical values.
In Details, Apply resets a malformed fixed value to zero for ground or ideal
meters and removes resistor-only fields from other component kinds. Opening
or dismissing Details leaves the stored fields untouched. A retained invalid
resistor sample must be repaired through a deliberate nominal resistance or
tolerance change before Apply can succeed.

With rotation 0, terminal 1 is two grid steps left of center and terminal 2
is two steps right. Ground has only terminal 1. A wire is a pair of distinct
points such as `[[0, 1], [2, 3]]`. Slanted segments are allowed.

Version 2 adds a `connections` object with two arrays. Its `terminals`
array parallels `components` and contains one node ID per terminal in
terminal order. Its `wires` array parallels `wires` and contains two node
IDs per segment in endpoint order. Node IDs are nonnegative integers.
Occurrences of the same ID must have the same grid coordinate. Distinct
IDs may occupy one coordinate and remain electrically separate. Each
segment electrically joins its two endpoint IDs. Connecting to a wire
interior splits that segment and gives both halves the new junction ID.
The optional `bends` array in `connections` lists node IDs that join
exactly two wire ends with no component terminal. They preserve the
geometry and connectivity of one wire path without showing a junction dot.

Version 1 documents without a `connections` object infer connectivity from coordinates. Coincident endpoints and terminals connect, as do endpoints and terminals lying on a segment.

For compatibility, Cirtam honors explicit node IDs in a version 1 document
that includes `connections` and writes version 2 on the next successful
save. A coordinate-only version 1 reader may ignore those IDs and join
deliberately separate endpoints at the same coordinate. A v1-only reader
cannot be relied on to preserve that circuit. Open the file in Cirtam and
save it as version 2 before exchanging it with a reader that supports
explicit connectivity.

Version 2 documents require explicit connections. Opening a coordinate-based
version 1 document in the editor materializes its inferred joins.

Files store circuit geometry and values. Selection, editor history, zoom,
and simulation results are session state.
