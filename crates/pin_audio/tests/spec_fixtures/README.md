# Spec-test fixtures (Subphase 3.3, Independent Tester)

`util.rs` is a helper module shared by the `spec_mixer*.rs` tests (included with
`#[path]`); WAV fixtures are generated in-test by its hand-written RIFF writer.

The two Ogg Vorbis files were produced offline with FFmpeg 6.1.1 (libvorbis
encoder, Ubuntu package `6.1.1-3ubuntu5`) from FFmpeg's own `sine` source, so
they contain no third-party audio:

```
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=0.25" \
  -af "volume=4" -ac 1 -c:a libvorbis -q:a 1 -bitexact -map_metadata -1 \
  sine440_mono_48k.ogg
ffmpeg -f lavfi -i "sine=frequency=880:sample_rate=44100:duration=0.2" \
  -af "volume=4,pan=stereo|c0=c0|c1=0*c0" -c:a libvorbis -q:a 1 -bitexact \
  -map_metadata -1 sine880_left_44k1.ogg
```

| file | channels | rate | length (final granule position) | signal |
|------|----------|------|---------------------------------|--------|
| `sine440_mono_48k.ogg` | 1 | 48 000 Hz | 12 000 frames (0.25 s) | 440 Hz sine, peak 0.5 |
| `sine880_left_44k1.ogg` | 2 | 44 100 Hz | 8 820 frames (0.2 s) | left: 880 Hz sine, peak 0.5; right: silence |

Note: the last Vorbis packet of each file decodes past the final granule
position (a decoder that does not trim to it yields 12 864 and 9 792 frames).
