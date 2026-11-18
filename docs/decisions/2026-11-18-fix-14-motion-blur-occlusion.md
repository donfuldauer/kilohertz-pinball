## fix-14-motion-blur-occlusion — motion blur over hidden balls deferred to render polish

**Decision:** Keep the disc-based motion blur. Blur is drawn from each ball's projected disc, so it
can show where the ball is hidden behind geometry. The limitation is documented on
`MotionBlurConfig` in `crates/pin_render/src/post/mod.rs`.

**Rationale:**
- The artefact is cosmetic.
- The fix needs a new depth-test or object-mask input to the post chain. That is a render-pipeline
  change, out of proportion for now.

**Revisit when:** render polish, or the artefact shows in a gate screenshot or in a player report.

**Alternatives:**
- Depth-test the blur against the scene depth buffer now: rejected for now, because the artefact is
  cosmetic only.

Source: review thread https://github.com/donfuldauer/kilohertz-pinball/pull/6#discussion_r4573647979.
