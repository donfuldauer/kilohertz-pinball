# Kilohertz Pinball UDP Telemetry Protocol, version 1

Status: **normative**. This is the written copy of PLAN.md Appendix B plus
the rules Subphase 3.4 fixed for the implementation (`pin_core::telemetry`
for the wire format, `pin_audio::telemetry` for the encoder and sender).
A change to anything in sections 2–4 is a breaking contract change
(`contracts/CHANGE_REQUESTS.md`) and needs a new `version`.

## 1. Transport

- UDP. Default destination `127.0.0.1:35120`; the destination is
  configurable (host and port).
- One datagram per sim tick that has changes; **no datagram** for a tick
  without changes.
- All multi-byte fields are **little-endian**. `f32` is IEEE 754 binary32.
- The sim runs at 1000 ticks per second; `tick` counts from 0 at the start of
  the sim session.
- Delivery is best effort (UDP): datagrams may be lost or reordered by the
  network. Listeners resynchronise from the periodic full state (section 4).

## 2. Datagram layout

A datagram is a 16-byte header followed by `entry count` 8-byte entries; its
length is exactly `16 + 8 × entry count` bytes (at most 1456).

### 2.1 Header (16 bytes)

| Offset | Size | Field       | Value                                      |
|-------:|-----:|-------------|--------------------------------------------|
| 0      | 4    | magic       | ASCII `"PNTL"` (`50 4E 54 4C`)             |
| 4      | 1    | version     | `u8` = 1                                   |
| 5      | 1    | flags       | `u8`; bit 0 = full state; bits 1–7 are 0   |
| 6      | 2    | entry count | `u16`, 0..=180                             |
| 8      | 8    | tick        | `u64`, the sim tick the entries describe   |

### 2.2 Entry (8 bytes)

| Offset | Size | Field    | Value                                  |
|-------:|-----:|----------|----------------------------------------|
| 0      | 1    | kind     | `u8`, see 2.3                          |
| 1      | 1    | reserved | `u8`; senders write 0, receivers ignore |
| 2      | 2    | id       | `u16`                                  |
| 4      | 4    | value    | `f32`                                  |

### 2.3 Kinds

| Kind | Name           | id                         | value                          |
|-----:|----------------|----------------------------|--------------------------------|
| 0    | switch         | switch number (0..=255)    | 0 open, 1 closed               |
| 1    | coil           | coil number (0..=255)      | 0 off, 1 energised             |
| 2    | lamp           | lamp number (0..=255)      | intensity 0..=1                |
| 3    | GI             | GI string (0..=31)         | intensity 0..=1                |
| 4    | magnet         | magnet number (0..=31)     | power 0..=1                    |
| 5    | haptic impulse | haptic source (section 5)  | strength 0..=1                 |

Kinds 0–4 are **states**: an entry says "from this tick on, element `id`
has `value`". Kind 5 is an **event**: an impulse that happened on this tick
(it has no persistent state).

Values are normalised by the sender: switches and coils are exactly 0.0 or
1.0; levels are clamped to `[0, 1]`, NaN and −0.0 are sent as +0.0.

## 3. Receiver rules

A receiver rejects (ignores) a datagram whose

- length is below 16 bytes,
- magic is not `"PNTL"`,
- version is not 1,
- entry count exceeds 180,
- length differs from `16 + 8 × entry count`, or
- any entry has a kind above 5.

The reference decoder (`pin_audio::telemetry::decode_datagram`, built on
`pin_core::telemetry::decode_datagram`) implements exactly these checks and
never panics on arbitrary bytes.

## 4. Sender rules

1. **Diff.** Each tick, the sender compares every element's normalised value
   with the previous tick's (bitwise) and emits one entry per element that
   changed.
2. **Impulses.** Every haptic impulse of the tick is emitted as a kind-5
   entry, at most 64 per tick (further impulses of that tick are dropped and
   counted by the sender).
3. **Order** within a tick: switches, coils, lamps, GI, magnets (each in
   ascending id), then impulses in the order they happened.
4. **Full state.** On table load, at the first tick of a session, whenever the
   tick goes backwards (a new session), and on every tick at least 1000 ticks
   (one second) after the previous full state, the sender emits **every**
   element — 256 switches, 256 coils, 256 lamps, 32 GI strings, 32 magnets
   (832 entries) — followed by that tick's impulses, and sets flags bit 0 on
   every datagram of that tick. A full state is sent even if nothing changed.
5. **Split.** A tick with more than 180 entries is sent as several datagrams
   with the same tick and flags, each with at most 180 entries, in order. A
   full state (832 entries) takes 5 datagrams, and so does a full state
   with the maximum 64 impulses (896 entries); no tick ever needs more than
   5 datagrams.
6. **Back-pressure.** The sim thread never blocks on the network: encoded
   datagrams go through a bounded lock-free ring to a telemetry thread that
   owns the socket. If the ring is full, datagrams are dropped (counted).
   The back-pressure loop is closed by the sim loop, not by the ring itself:
   when `TelemetryTx::push_batch` returns `false` the sim loop must call
   `TelemetryEncoder::force_full_state()`, so the next tick sends a full
   state and listeners resynchronise. Integration (6.0) must wire this:

   ```rust
   encoder.encode_tick(tick, &state, &impulses, &mut batch);
   if !telemetry_tx.push_batch(&batch) {
       encoder.force_full_state();
   }
   ```
7. **Latency.** The telemetry thread (`TelemetrySender::spawn(poll)`) parks
   for up to `poll` when the ring is empty, so a datagram can leave up to
   `poll` after its tick was encoded (plus OS scheduling). Integration (6.0)
   should use `poll ≤ 1 ms` (one tick); the network layer adds nothing
   else.

## 5. Haptic source ids (kind 5)

The id of a haptic impulse identifies what caused it. The top four bits name
the class, the low twelve bits the element number (modulo 4096):

| Class (bits 15–12) | Source                            | Low 12 bits       |
|-------------------:|-----------------------------------|-------------------|
| 0                  | coil fire (flipper, sling, ...)   | coil number       |
| 1                  | flipper end-of-stroke hit         | flipper number    |
| 2                  | pop bumper                        | bumper mech index |
| 3                  | ball drop (landing)               | ball slot         |
| 4                  | (ball roll; never sent as kind 5) | ball slot         |

Impulses from scripts or tools that carry a raw source id
(`HapticsTelemetryEvent`) are sent with that id unchanged. Continuous ball
roll is not an impulse and is not part of the telemetry stream.

## 6. Listener guidance

- Keep a mirror of all state elements; apply every kind 0–4 entry of every
  accepted datagram.
- On a full-state datagram, treat its entries as authoritative; until the
  first full state arrives, the mirror is unknown.
- Datagrams of one tick may arrive out of order relative to other ticks;
  ignore datagrams whose tick is below the last full-state tick seen,
  **unless the datagram is itself a full state**: a full state with a lower
  tick means a new session (section 4, rule 4) and replaces the mirror.
- Example (lamp 1 at full, tick `0x1122334455667788`, full-state flag; a
  real full state would carry all 832 entries over 5 datagrams):

  ```text
  50 4E 54 4C 01 01 01 00 88 77 66 55 44 33 22 11  02 00 01 00 00 00 80 3F
  ```
