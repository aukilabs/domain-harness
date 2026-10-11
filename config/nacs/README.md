# NACS

New physical demo environment, separate from **Xmart**.

The user designated **RG3E3LUITXC**, supplied size **15 cm**, as the origin
Portal. Its center is `(0, 0, 0)` in NACS. `portals.json` records the supplied
size as `0.15` meters with confirmed `encoded_square` measurement basis.
The user's standing convention is that supplied Portal sizes always measure
the encoded square itself, excluding the white border; do not ask again for
each Portal unless the user explicitly changes that convention.

The user confirmed the origin Portal lies flat on the floor. Its printed face
points upward (+Y), with its center on the local floor plane at Y=0. A -90 degree
rotation about X maps the Portal's right edge to +X and its top to -Z. This defines
the map's horizontal heading, not a geographic bearing. Domain association and
other Portal poses remain unspecified. No Xmart geometry has been copied.

`config/environments.json` records both names and their separate input files.
Xmart remains the active reference scene with its existing IDs, frame, journal,
Portal geometry, and sizes. NACS is independently visible under Local in Maps
and exported as a standard SDK Map Product by the `local-scene-nacs` component.
It is read-only: selecting it does not switch the existing Xmart calibration,
mapping, or annotation pipeline. Multi-scene contribution targeting remains future work.

Home loads saved native SDK MapSnapshots from the journal's sibling `.scenes`
directory (normally `data/home-portals.scenes/`). It seeds NACS only when absent;
existing saved data is authoritative. Additional valid OpenGL-meter snapshots in
that directory are listed automatically after Home restarts, with independent
IDs and frames. The environment catalog's `preferred_scene` chooses the initial
UI selection outside synthetic demo mode; polling preserves the user's selection.
Home's network authorization domain is unchanged.

For each additional Portal, provide its code and encoded-square side length
with units. Domain/pose information can follow separately; do not treat an
unknown pose as identity.
