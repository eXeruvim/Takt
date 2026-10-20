# Takt: pinned libmpv-sys source

Copied from `ParadoxSpiral/libmpv-rs` commit
`3e6c389b716f52a595cc5e8e3fa1f96cb76b3de7` for offline, reproducible builds.
The upstream license, headers, pregenerated bindings and copyright notices are
retained.

The only source-package change is the license metadata spelling:
`LGPL-2.1-or-later` replaces the deprecated ambiguous SPDX identifier
`LGPL-2.1`, matching the unchanged source headers and license text. Vendoring
this package also prevents a build from silently depending on the mutable state
of a local Cargo Git checkout.
