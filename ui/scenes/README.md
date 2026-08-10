# OWI scene compositions

Scene JSON composes reusable asset IDs without owning application truth. A
renderer selects one layout profile, creates an isolated stacking context,
places instances inside the declared z-index bands, and keeps live HTML above
the art. `state_binding` names are presentation inputs only; missing state art
must fall back to the registered idle asset.

The scene contract uses normalized `[x, y, width, height]` bounds. Profiles are
resolved in this order when multiple media conditions match:
`phone-landscape`, `phone-portrait`, `tablet`, `desktop`. Phone portrait keeps
the wide training map at 16:9 and puts tutorial controls below it; it must not
crop the route into an unreadable portrait background.

Hotspots are semantic placement hints. They share stable `target_anchor`
names with the DOM tour anchors, but they do not perform actions. The
application decides whether a view change, model run, outcome, or PR action is
allowed.
