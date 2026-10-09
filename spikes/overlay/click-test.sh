#!/usr/bin/env bash
# Manual check of the partially interactive variant (30 s, bottom-right 320x320): click the blue button at the
# lower left of the surface; it turns amber and the event is printed here. Move over the rest of the ring: no event
# should arrive (the rest of the surface is outside the input region). Needs a human; not automated on purpose.
cd "$(dirname "$0")" && exec ./target/release/overlay --mode click --interactive --bg alpha --scenario speaking --secs 30
