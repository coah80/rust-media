# Vendored HEVC decoder

`rusty_h265` and `rusty_h265-accel` contain library source from their 0.6.0 crates.io releases, at upstream revision `9d04cf605a5b1b55dabec089a07b5c122b4c3c81` in [Remade-With-Rust/rusty_h265](https://github.com/Remade-With-Rust/rusty_h265). Both use the Apache-2.0 license. License files are included in each directory.

The upstream luma deblocking bounds check requires `x >= 4` for both edge directions. Horizontal edges at column zero are valid because their filter taps extend vertically. This copy checks the direction before requiring a four-pixel margin and uses checked arithmetic for the footprint. The regression check compares the horizontal border kernel with its scalar implementation.

The manifests include only library targets and point the decoder at the local accelerator crate. Source formatting follows rustfmt. The upstream command-line program, integration fixtures, and package metadata are omitted.

These are direct path dependencies so the fix also applies when another app depends on Rust Media. Remove these copies when a verified upstream release includes the fix.
