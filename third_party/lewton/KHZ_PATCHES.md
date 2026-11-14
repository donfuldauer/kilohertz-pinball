# Kilohertz Pinball patches to lewton 0.10.2

Vendored from crates.io `lewton =0.10.2` and wired in with `[patch.crates-io]` in
the workspace `Cargo.toml`. Tables are untrusted downloads (D25): a few bytes of
header could make upstream lewton reserve gigabytes, and an allocation failure
aborts the process (no unwinding). Every edit is marked `KHZ` in the source.
Rationale: docs/DECISIONS.md, "3.3 — fix round 1".

| # | File | Patch |
|---|------|-------|
| 1 | `src/header.rs`, `src/huffman_tree.rs` | Header-driven `Vec::with_capacity(n)` (comment list, codeword lengths, VQ multiplicands, VQ value vectors, Huffman `desc_prog`) reserves at most `KHZ_MAX_PREALLOC` (4096) elements up front (`khz_cap`); the vectors still grow on push. The remaining `with_capacity` calls are bounded by ≤ 8-bit header fields. |
| 2 | `src/header.rs` | Per setup header, the sum of codebook entries is at most `KHZ_MAX_CODEBOOK_ENTRIES` (2^18) and the sum of VQ lookup scalars (entries × dimensions, for codebooks with a lookup table) at most `KHZ_MAX_VQ_SCALARS` (2^20); otherwise `HeaderBadFormat`. These are the only setup-header allocations not paid for with input bits (ordered codeword lengths, computed lookup vectors, Huffman tree nodes). libvorbis 1.3.7 files (q −1…10, 8–96 kHz, mono and stereo) measured at most 11 813 entries and 61 254 scalars (≤ 6 % of either budget). |
| 3 | `src/header.rs` | Comment header: a vendor or comment length longer than the rest of the packet is `HeaderBadFormat` before its buffer is allocated. (`pin_audio` does not parse the comment header at all.) |
| 4 | `src/lib.rs` | `#![allow(unexpected_cfgs, dead_code, mismatched_lifetime_syntaxes)]`: as a path dependency its warnings are not capped by cargo, and rustc 1.97 warns on this 2015-edition code. No behaviour change. |
| — | `Cargo.toml` | Examples and dev-dependencies removed; `autoexamples`/`autotests`/`autobenches` off. `.github`, `examples`, `Cargo.lock`, `cbindgen.toml` and editor files removed. |

Audio-packet decoding (`src/audio.rs`) allocates per packet in proportion to the
block size (≤ 8192, validated in the identification header) and the channel
count (`pin_audio` rejects more than two channels before reading the setup
header), so it is unpatched.

