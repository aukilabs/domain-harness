# BB Lab

Lab environment for Bracketbot testing, separate from **Xmart**, **NACS** and
**Humanoid Hub Conference**.

Origin: Portal `280UO1U1RPC`, supplied size **15 cm**, recorded in `portals.json`
as `0.15` meters measured across the encoded square (excluding the white border).
Its center is `(0, 0, 0)` in this scenegraph.

The Portal lies flat, printed face up (+Y), on a **table about 0.7 m above the
floor**: Y=0 is the table top, not the floor. A Bracketbot aligned to it measured
its floor at Y ≈ -0.69. Right is +X and top is -Z (-90 degree rotation about X,
same as NACS and the conference). If the Portal moves, update `origin` in
`config/environments.json` and delete `data/home-portals.scenes/bb-lab.json` so
Home reseeds it.

Domain association and other Portal poses are unspecified.
