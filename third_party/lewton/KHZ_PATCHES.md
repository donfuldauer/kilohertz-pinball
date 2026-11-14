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
| 5 | `src/header.rs`, `src/audio.rs`, `src/huffman_tree.rs`, `src/bitpacking.rs`, `src/samples.rs`, `src/inside_ogg.rs`, `src/imdct_test.rs` | Malformed-stream fixes (CodeRabbit review of PR #3); each used to panic, hang or misdecode. Setup header: floor 0 with order, rate or bark-map size 0 is `HeaderBadFormat` (as libvorbis; a zero rate or bark-map size made the cached cos(ω) NaN), and a floor 0 book index equal to the codebook count is rejected (was `>`); a codebook with a lookup table and 0 dimensions is rejected (as stb_vorbis; residue type 0 divided by it; a scalar-only book with 0 dimensions is still accepted, and `residue_packet_decode_inner` in `audio.rs` rejects it at use if it is a residue classbook and residue data needs decoding); ordered codeword lengths past 32 are rejected (the `u8` length overflowed); residue cascades read all 8 stage books (upstream read stages 0–6, so a stage-7 bit shifted every later setup field). Audio: floor 0 curve loop counts are `order / 2` and `(order + 1) / 2` (same as the spec's bounds, without the `order - 3` underflow at order 1), and its output loop always advances (a NaN cos(ω) never compared equal and hung); `dual_mut_idx` returns channels in argument order (magnitude/angle were swapped when magnitude > angle); `get_decoded_sample_count` returns `AudioBadFormat` for an unused mode number instead of indexing; decoding from a book with no used entries is an end-of-packet error instead of an out-of-bounds index. `InterleavedSamples::truncate` clamps before multiplying by the channel count. `OggStreamReader` takes a chained link's page granule as its position only when the priming packet ends the page. The IMDCT test helper counts non-finite values as mismatches. Regression tests: `khz_tests` modules in `header.rs` and `audio.rs`, `khz_*` tests in `huffman_tree.rs`, `samples.rs`, `imdct_test.rs`. |
| — | `Cargo.toml` | Examples and dev-dependencies removed; `autoexamples`/`autotests`/`autobenches` off. `.github`, `examples`, `Cargo.lock`, `cbindgen.toml` and editor files removed. |

Audio-packet decoding (`src/audio.rs`) allocates per packet in proportion to the
block size (≤ 8192, validated in the identification header) and the channel
count (`pin_audio` rejects more than two channels before reading the setup
header), so it is not patched for allocation (patch 5 fixes its malformed-input panics).

