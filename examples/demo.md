# Sample corpus — a tiny fake "engine manual" to try md-doc-search instantly.

Run: `md-doc-search examples/demo.md "boolean modifier"`

---

# Boolean Modifier

The Boolean modifier performs boolean operations on meshes. The solver used for
the operation is the Exact solver, which is slower but always produces a
correct result, unlike the Fast solver based on floating point collections.

## Options

### Solver

- **Exact**: voxel-free, robust, handles self-intersections.
- **Fast**: quicker but may produce invalid geometry on non-manifold meshes.

### Operation

Difference, Union, Intersect. The boolean result depends on operand normals.

---

# Volume Scatter

The Volume Scatter shader scatters light coming from behind the surface.
Density controls how much light is scattered; Anisotropy (Henyey-Greenstein)
controls the direction: positive values scatter forward, negative values
scatter backward.

### Density

Higher density means thicker fog and shorter visible light shafts.

---

# Mirror Modifier

The Mirror modifier mirrors a mesh across its local X, Y or Z axis, across
another object, and can merge vertices whose distance falls under the merge
threshold. Use clipping to keep the seam welded while editing.

**merge_threshold**(distance: float) — vertices closer than this distance are
merged across the mirror boundary.
