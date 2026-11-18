## fix-10-srgb-blend — where display layers meet an `*Srgb` surface (cause)

**Decision:** The cause in issue #10 is confirmed in the display code, but the surface it was
attributed to is not where it happens. `QuadRenderer` and `DmdRenderer` (`displays/quad.rs`,
`displays/dmd.rs`) are built for the target format; for an `*Srgb` format they set `srgb` and the
shader decodes its output (`encode_output`), but their pipelines blend with fixed-function
`PREMULTIPLIED_ALPHA_BLENDING` into the `*Srgb` view, which decodes the destination, blends in linear
space and re-encodes. `pin_render`'s `SurfaceSet` (`gpu/surfaces.rs`) picks the `*Srgb` fallback in
`pick_format` and configures it with `view_formats: Vec::new()`, but it draws only debug lines and the
secondary clear; it never draws display layers. The production path that draws display layers onto a
surface is `pin_app`'s table renderer (`crates/pin_app/src/orchestrator/present.rs`: its own
`pick_format` with the same `*Srgb` fallback, a `SurfaceConfiguration` with `view_formats: Vec::new()`,
`TableRenderer::new(ctx, format, ..)`, and `render_direct` drawing everything through the default
`*Srgb` view of the swap-chain texture). `pin_app` is outside this unit's owned paths, so this unit
fixes the mechanism in `pin_render` and the `SurfaceSet` configuration, and the `pin_app` wiring is a
request (queue file).

**Rationale:** Fixing only what the issue names would leave production unchanged, and a silent
partial fix would close the issue wrongly. The brief says to fix the real cause; the part of it in
another crate is the Orchestrator's call.

**Alternatives:** Edit `pin_app` here (rejected: outside OWNED PATHS); declare the issue invalid
because `SurfaceSet` draws no layers (rejected: production `pin_app` has the bug).

## fix-10-srgb-blend — approach: twin-format view, scratch texture where views are unsupported

**Decision:** New public `pin_render::gpu::LayerTarget` and `LayerCompositor` (in
`gpu/surfaces.rs`). `LayerTarget::for_target(format, downlevel_flags, usages)` decides how display
layers reach a target texture:

- non-sRGB format → `Direct(format)` (unchanged: no extra view, pass, copy or allocation);
- `*Srgb` format and `DownlevelFlags::SURFACE_VIEW_FORMATS` → `TwinView`: the surface is configured
  with `view_formats = [format.remove_srgb_suffix()]`, display renderers are built for the twin
  (`layer_format()`; their `srgb` flag is then false, so no decode), and `LayerCompositor::begin`
  returns a twin-format view of the frame texture: blending happens on display-encoded bytes,
  identical to the non-sRGB path, with no extra pass;
- `*Srgb` without `SURFACE_VIEW_FORMATS` but with `COPY_SRC | COPY_DST` → `Intermediate`: the
  surface is configured with those usages; `begin` copies the frame into a twin-format scratch
  texture (a raw byte copy; wgpu allows copies between formats that differ only in sRGB-ness),
  layers are drawn there, `end` copies it back. The scratch texture is created on first use and
  again only on a size change;
- neither → `Direct(srgb format)`: the pre-fix behaviour (linear blend) as a documented last resort.

`SurfaceSet::new` calls `for_target` for each direct surface (presenter-thread surfaces draw into an
`Rgba8Unorm` offscreen texture, so they are `Direct(Rgba8Unorm)`), passes the result to
`surface_config` (view formats and extra usages), and exposes it as `SurfaceSet::layer_targets()`.
The debug renderers keep their `draw_format` and default view, so their output is unchanged on every
path.

**Rationale:** The brief prefers the issue's first option. wgpu 30.0.1 (the pinned version) accepts
surface `view_formats` only that differ in sRGB-ness and only with
`DownlevelFlags::SURFACE_VIEW_FORMATS` (wgpu-core `device/resource.rs`, `configure`): DX12 and Metal
report it; Vulkan reports it only with `VK_KHR_swapchain_mutable_format`; GLES and Vulkan on Android
do not. So the first option is unsupported on some backends, and the brief's second option covers
them. The copy-in/copy-out form of the second option keeps whatever was drawn under the layers
(playfield, post output) without making the rest of the frame change format.

**Alternatives:** Twin view only, failing configuration elsewhere (rejected: a surface that worked
before would stop working); draw the whole frame in the twin format (rejected: changes debug lines and
post output on the fallback path, which the brief keeps as before); decode-blend-encode in the shader
with a manual read of the destination (rejected: needs framebuffer fetch or a copy anyway).

## fix-10-srgb-blend — `encode_output` and the `srgb` uniform stay

**Decision:** `encode_output` and the `srgb` uniform in `quad.rs` and `dmd.rs` are kept unchanged.
After the fix no `SurfaceSet` path builds a display renderer for an `*Srgb` format, and once `pin_app`
adopts `LayerTarget` it will not either, except in the degraded `Direct(*Srgb)` case, which still
needs the decode so unblended stored values match. Clippy does not flag them.

**Rationale:** The brief asks to keep them working for any path that still renders to a true `*Srgb`
view; the degraded case and the new test's pre-fix comparison are such paths.

**Alternatives:** Remove the uniform (rejected: breaks the degraded case and the brief's rule 4).

## fix-10-srgb-blend — test tolerance

**Decision:** `tests/srgb_blend_gpu.rs` compares the raw stored bytes of the `*Srgb` target (through
the production `for_target` choice, plus `Intermediate` and, where `DownlevelFlags::VIEW_FORMATS`
allows texture view formats, `TwinView` forced) with the non-sRGB twin's within 1 LSB per channel, and
region means with the analytic display-encoded alpha-over within 1 LSB, for four background/layer
pairs on `Rgba8` and `Bgra8`. A second test shows the pre-fix path (renderer built for the `*Srgb`
format, drawn through the `*Srgb` view) matches the analytic linear-space blend within 2 LSB and
differs from the twin by at least 40 LSB (red at 128/255 over blue: blue channel 187 instead of 127).
The brief's example pair (white at 0.5 over black, 188 vs 128) does not discriminate on this code:
the pre-fix shader decodes its premultiplied output before the blend, and over a black destination
`encode(decode(s) + 0) = s`, so both paths store 128. Three of the four pairs therefore use a
non-black destination (blue clear; grey and orange opaque base layers).

**Rationale:** Both sides store the same blend of the same 8-bit values in `*Unorm` arithmetic; 1 LSB
allows for a backend that rounds differently between a twin view and a plain texture, while linear
blending is about 60 LSB off at mid-tones, so the tolerance cannot accept it.

**Alternatives:** Exact equality (rejected: no spec guarantees identical rounding across the two
views); snapshot images (rejected: D30, analytic first).

## fix-10-srgb-blend (part 2) — pin_app wiring

**Decision:** `pin_app`'s table presentation (`crates/pin_app/src/orchestrator/present.rs`) now
computes one `LayerTarget` per slot at configure time (`slot_layers`, pure and unit-tested) and keeps
it, in a `LayerCompositor`, with the slot's state for its lifetime: resizing changes only the
surface's width and height, never its format or capabilities, so nothing is recomputed there (the
`Intermediate` scratch texture is resized by the compositor itself on the first frame of a new size).

- **Direct slots** (the playfield, non-blocking secondary windows): `LayerTarget::for_target(format,
  adapter downlevel flags, caps.usages)`; the surface is configured with `view_formats =
  lt.view_formats()` and `usage = RENDER_ATTACHMENT | lt.extra_usages()`. `TableRenderer` is built
  with `TableRenderer::with_layer_format(ctx, format, lt.layer_format(), ..)` (`view.rs`): the score
  display and backglass renderers are built for the layer format, the post chain and the debug overlay
  for the surface format. Each renderer owns its pipelines, so the display and post pipelines cannot
  be mixed up (no pipeline cache is shared between them). A frame is recorded by
  `TableRenderer::encode_composited`: PBR, post and debug overlay into the frame's own view, then
  `LayerCompositor::begin(device, encoder, &frame.texture)` (falling back to the frame's own view on
  `None`), the displays, and `LayerCompositor::end`, all before the submit. The brief's "call `end`
  before post and debug" is read as "post and debug are not drawn through the layer view": the draw
  order stays as before (post, debug, then displays on top), because `begin` must see the finished
  frame content for `Intermediate` to copy it in under the layers.
- **Presenter-thread slots** (requirement 4): **a twin-format offscreen.** The render thread draws
  the frame into `offscreen_frame_texture(offscreen_format(format))`, where `offscreen_format` strips
  the sRGB suffix, and the renderer is built entirely for that format. Its `LayerTarget` is
  `for_target(twin, ..) = Direct(twin)` (`begin` returns `None`, no extra pass), and the surface is
  configured as before (copy-path usages, `view_formats` from that `Direct` target, which is empty).
  The presenter's copy into the `*Srgb` swap-chain image moves the bytes unchanged:
  `copy_texture_to_texture` accepts formats that differ only in sRGB-ness (wgpu-core 30.0.1
  `command/transfer.rs`) and copies raw bytes; the blit path (surfaces without `COPY_DST`) now uses
  `FrameCopy::with_source(device, twin, surface_format, path)`, whose fragment entry `fs_decode`
  decodes the sampled (display-encoded) colour before the `*Srgb` target encodes it again, so no
  double encode happens (an 8-bit decode→encode round trip returns every byte; unit test). The secondary
  window's black clear is the same bytes in either format.
- **Non-sRGB surfaces**: `for_target` gives `Direct(format)`, `offscreen_format` returns the format
  unchanged, `view_formats` is empty and `usage` is as before; `begin` and `end` return without
  recording or allocating, and the blit keeps `fs`. No texture, view, pass or heap allocation is added
  per frame. A frame without any display routed to its window skips `begin`/`end` too.
- `TableRenderer::render_to_image` (tests) uses the same `encode_composited`, with `Direct` when the
  layer format equals the target format (unchanged) and `Intermediate` otherwise (texture view
  formats would need `DownlevelFlags::VIEW_FORMATS`; copies work everywhere). `TableRenderer::new`
  and `encode` keep their old behaviour (layer format = target format).
- The side finding about `pin_render`'s `SurfaceSet` presenter-thread blit (`gpu/blit.rs`:
  `textureLoad` of the `Rgba8Unorm` offscreen into the surface's default view) does not apply to
  `pin_app`, whose presenter path is handled above. In `SurfaceSet` it is not a defect either: that
  offscreen holds only debug lines and the secondary clear, drawn as raw values, and the `*Srgb` write
  encodes them exactly as the direct `*Srgb` path does when the debug renderer draws through the
  surface's `*Srgb` view, so both paths store the same bytes. No queue line or request.

**Rationale:** The twin-format offscreen needs no downlevel flag, no extra usage, no extra pass and no
per-frame view on the render thread, and its frame is already display-encoded; giving the offscreen
the same `LayerTarget` treatment as the surface would need `DownlevelFlags::VIEW_FORMATS` on the pool
textures (or a copy in and out per frame) to get the same bytes. A presenter-thread window never
draws the playfield, so building its whole renderer in the twin format changes nothing but the
display blend.

**Alternatives:** Same treatment for the offscreen (`*Srgb` pool texture with twin view formats or a
scratch copy; rejected: needs a flag or two copies per frame for an identical result); blit into a
twin-format view of the swap-chain image (rejected: needs `SURFACE_VIEW_FORMATS`, which the fallback
path cannot assume; the shader decode works everywhere); recompute the `LayerTarget` on every
reconfigure (rejected: its inputs cannot change after configuration).
