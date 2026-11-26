//! The audio thread's engine and output (Subphase 6.0 wiring).
//!
//! * [`AudioEngine`]: one block at a time, the 3.3 [`Mixer`] (sound
//!   triggers from the sim's `HostCommandQueue::sounds`, through the
//!   wait-free [`SoundSender`](pin_audio::mixer::SoundSender) ring that
//!   [`TableSim`](super::TableSim) feeds) renders stereo, the 3.4
//!   [`SsfMixer`] renders the exciter channels from the haptic impulses, and
//!   [`SsfMixer::render_with_stereo`] combines both into one interleaved 7.1
//!   block (stereo on FL/FR, exciters on SL/SR/BL/BR).
//! * [`AudioSink`]: where the blocks go. [`NullSink`] discards them,
//!   [`OfflineSink`] records them into memory (tests, D31: audio renders
//!   offline), [`DeviceSink`] pushes them to an SDL3 playback stream
//!   opened on the main thread ([`DeviceStream::open`]; SDL converts 7.1 to
//!   the device's channel layout). No device, or `--headless`, falls back
//!   to [`NullSink`].
//! * [`table_samples`]: decodes the table's `[[sounds]]` from the validated
//!   bundle snapshot for [`Mixer::register`].
//! * UDP telemetry (SPEC §4 3.4, `docs/TELEMETRY_PROTOCOL.md`, KP-006):
//!   [`TelemetryFeed`] runs on the sim thread inside
//!   [`TableSim`](super::TableSim): each tick it fills a [`TelemetryState`]
//!   from the snapshot and the sim's state ([`TelemetrySources`]) through a
//!   [`TelemetryMap`] (table `number`s on the wire, interned ids inside),
//!   encodes it and pushes the datagrams through the wait-free ring that
//!   [`open_telemetry`] created; the sender thread behind the returned
//!   [`TelemetryLink`] owns the socket.

use super::table::ValidatedTable;
use pin_assets::IdKind;
use pin_audio::mixer::{
    Bus, DecodeBudget, DecodeError, Mixer, MixerConfig, Sample, SoundReceiver, decode_with_budget,
    trigger_ring,
};
use pin_audio::ssf::{CHANNELS, SsfConfig, SsfMixer};
use pin_audio::telemetry::{
    DEFAULT_TELEMETRY_RING_CAPACITY, DatagramBatch, GI_COUNT, HapticsTelemetryEvent, MAGNET_COUNT,
    TelemetryEncoder, TelemetrySenderHandle, TelemetryState, TelemetryTx, telemetry_channel,
};
use pin_core::FrameStateSnapshot;
use pin_core::SoundId;
use pin_core::haptics::HapticEvent;
use pin_core::schema::table::{LampKind, TableManifest};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

/// Frames the audio thread renders per block (5.3 ms at 48 kHz).
pub const AUDIO_BLOCK_FRAMES: usize = 256;

/// A sound registered with the mixer: id, decoded sample, bus.
pub type RegisteredSound = (SoundId, Arc<Sample>, Bus);

/// Decodes every `[[sounds]]` entry of `checked` from its bundle snapshot
/// at `sample_rate`, in manifest order, against one
/// [`DecodeBudget::new`]`(decoded_budget_bytes)` for the whole table (the
/// player's `audio.decoded_budget_mib` setting, SPEC §4 4.4; D25).
///
/// Returns the sounds in id order and exactly one message per sound that
/// could not be read or decoded; that sound stays silent and the load goes
/// on. Sounds that fit keep their charge, so once the budget is spent every
/// later sound that does not fit fails with
/// `sound <id> (<file>): decoded-audio budget of <N> MiB exhausted; raise
/// audio.decoded_budget_mib` (`N = decoded_budget_bytes >> 20`); a failed
/// decode leaves nothing charged.
pub fn table_samples(
    checked: &ValidatedTable,
    sample_rate: u32,
    decoded_budget_bytes: u64,
) -> (Vec<RegisteredSound>, Vec<String>) {
    let budget = DecodeBudget::new(decoded_budget_bytes);
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for def in &checked.table.manifest.sounds {
        let Some(id) = checked
            .table
            .ids
            .entries(IdKind::Sound)
            .iter()
            .find(|(n, _)| *n == def.id)
            .map(|&(_, i)| SoundId(i))
        else {
            errors.push(format!("sound {}: no id", def.id));
            continue;
        };
        let sample = checked
            .snapshot
            .read(&def.file)
            .map_err(|e| e.to_string())
            .and_then(|b| {
                decode_with_budget(&b, sample_rate, &budget).map_err(|e| match e {
                    DecodeError::BudgetExhausted => format!(
                        "decoded-audio budget of {} MiB exhausted; raise audio.decoded_budget_mib",
                        decoded_budget_bytes >> 20
                    ),
                    other => format!("{other:?}"),
                })
            });
        match sample {
            Ok(s) => out.push((id, Arc::new(s), Bus::from(def.bus))),
            Err(e) => errors.push(format!("sound {} ({}): {e}", def.id, def.file)),
        }
    }
    out.sort_by_key(|(id, _, _)| id.0);
    (out, errors)
}

/// The audio thread's mixing state: the 3.3 mixer and the SSF exciter
/// mixer combined into interleaved 7.1 blocks of [`AUDIO_BLOCK_FRAMES`].
/// All buffers are allocated in [`new`](Self::new);
/// [`render_block`](Self::render_block) never allocates.
pub struct AudioEngine {
    ssf: SsfMixer,
    mixer: Mixer,
    stereo: Vec<f32>,
    block: Vec<f32>,
    peak_voices: usize,
}

impl AudioEngine {
    /// An engine whose mixer takes its triggers from `sounds` (`None`: no
    /// trigger source, the mixer stays silent) and plays `samples`.
    pub fn new(
        ssf: SsfConfig,
        mixer: MixerConfig,
        sounds: Option<SoundReceiver>,
        samples: &[RegisteredSound],
    ) -> AudioEngine {
        let rx = sounds.unwrap_or_else(|| trigger_ring(1).1);
        let mut mixer = Mixer::new(mixer, rx);
        for (id, s, bus) in samples {
            mixer.register(*id, Arc::clone(s), *bus);
        }
        AudioEngine {
            ssf: SsfMixer::new(ssf),
            mixer,
            stereo: vec![0.0; AUDIO_BLOCK_FRAMES * 2],
            block: vec![0.0; AUDIO_BLOCK_FRAMES * CHANNELS],
            peak_voices: 0,
        }
    }

    /// Registers the playfield position `[x, y]` (mm, the
    /// [`SsfConfig::playfield_mm`] frame) of each haptic source id with the
    /// SSF exciter mixer, so its impulses are panned there instead of at
    /// the playfield centre (load time only: allocates). Call before the
    /// first [`render_block`](Self::render_block).
    #[must_use]
    pub fn with_source_positions(mut self, positions: &[(u16, [f32; 2])]) -> AudioEngine {
        for &(source, [x, y]) in positions {
            self.ssf.set_source_position(source, x, y);
        }
        self
    }

    /// Feeds one haptic impulse to the SSF exciter mixer.
    pub fn feed_haptic(&mut self, e: HapticEvent) {
        self.ssf.feed(e);
    }

    /// Renders the next block: the mixer (draining every pending sound
    /// trigger first) into stereo, then the SSF channels with the stereo on
    /// FL/FR. Returns the interleaved 7.1 block ([`CHANNELS`] per frame).
    pub fn render_block(&mut self) -> &[f32] {
        self.mixer.render(&mut self.stereo);
        self.peak_voices = self.peak_voices.max(self.active_voices());
        self.ssf.render_with_stereo(&self.stereo, &mut self.block);
        &self.block
    }

    /// The last rendered block.
    pub fn block(&self) -> &[f32] {
        &self.block
    }

    /// Voices playing on all mixer buses now.
    pub fn active_voices(&self) -> usize {
        Bus::ALL.iter().map(|&b| self.mixer.active_voices(b)).sum()
    }

    /// Most mixer voices seen still playing at the end of a block (a voice
    /// that starts and ends within one block is not counted).
    pub fn peak_voices(&self) -> usize {
        self.peak_voices
    }

    /// The 3.3 mixer.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// The SSF exciter mixer.
    pub fn ssf(&self) -> &SsfMixer {
        &self.ssf
    }
}

/// The SSF playfield rectangle `[x_min, x_max, y_min, y_max]` (mm) of a
/// table whose plan bounds are `(min, max)` in metres
/// ([`TableVisuals::bounds`](super::TableVisuals::bounds)), so source
/// positions in the table frame pan across the exciters. A degenerate or
/// non-finite rectangle keeps the [`SsfConfig`] default.
pub fn ssf_playfield_mm(bounds: ([f32; 3], [f32; 3])) -> [f32; 4] {
    let (a, b) = bounds;
    let r = [a[0], b[0], a[1], b[1]].map(|v| v * 1000.0);
    if r.iter().all(|v| v.is_finite()) && r[1] > r[0] && r[3] > r[2] {
        r
    } else {
        SsfConfig::default().playfield_mm
    }
}

/// Where the audio thread sends its 7.1 blocks.
pub trait AudioSink: Send {
    /// Short name for logs (`null`, `offline`, `device`).
    fn name(&self) -> &'static str;

    /// `true` for a sink that paces the audio thread (a device): the thread
    /// then renders while [`wants_block`](Self::wants_block) says so.
    /// Unpaced sinks get one block per audio-thread period.
    fn paced(&self) -> bool {
        false
    }

    /// For a paced sink: whether it has room for another block now.
    fn wants_block(&mut self) -> bool {
        true
    }

    /// Takes one interleaved block of `channels` channels.
    fn write(&mut self, block: &[f32], channels: usize);
}

/// Discards every block (headless, or no output device), counting frames.
#[derive(Debug, Default)]
pub struct NullSink {
    frames: u64,
}

impl NullSink {
    /// Frames written so far.
    pub fn frames(&self) -> u64 {
        self.frames
    }
}

impl AudioSink for NullSink {
    fn name(&self) -> &'static str {
        "null"
    }

    fn write(&mut self, block: &[f32], channels: usize) {
        self.frames += (block.len() / channels.max(1)) as u64;
    }
}

/// The shared buffer of an [`OfflineSink`]: read it after the audio thread
/// was joined (or at any time; a mutex guards it — on the audio thread,
/// never on the sim thread).
#[derive(Clone, Debug, Default)]
pub struct OfflineRecording {
    data: Arc<Mutex<Vec<f32>>>,
}

impl OfflineRecording {
    /// A copy of the recorded interleaved samples.
    pub fn samples(&self) -> Vec<f32> {
        self.data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Largest absolute sample recorded so far.
    pub fn peak(&self) -> f32 {
        self.data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .fold(0.0f32, |m, &x| m.max(x.abs()))
    }
}

/// Records blocks into memory, up to a fixed number of frames (the rest is
/// dropped). Unpaced.
#[derive(Debug)]
pub struct OfflineSink {
    data: Arc<Mutex<Vec<f32>>>,
    max_samples: usize,
    channels: usize,
}

impl OfflineSink {
    /// A sink keeping at most `max_frames` frames of [`CHANNELS`] channels,
    /// and the handle to read them.
    pub fn new(max_frames: usize) -> (OfflineSink, OfflineRecording) {
        let max_samples = max_frames.saturating_mul(CHANNELS);
        let data = Arc::new(Mutex::new(Vec::with_capacity(max_samples)));
        (
            OfflineSink {
                data: Arc::clone(&data),
                max_samples,
                channels: CHANNELS,
            },
            OfflineRecording { data },
        )
    }
}

impl AudioSink for OfflineSink {
    fn name(&self) -> &'static str {
        "offline"
    }

    fn write(&mut self, block: &[f32], channels: usize) {
        if channels != self.channels {
            return;
        }
        let mut d = self.data.lock().unwrap_or_else(PoisonError::into_inner);
        let room = self.max_samples.saturating_sub(d.len());
        d.extend_from_slice(&block[..block.len().min(room)]);
    }
}

/// Raw SDL stream pointer shared by [`DeviceStream`] (main thread) and
/// [`DeviceSink`] (audio thread).
struct RawStream(*mut sdl3::sys::audio::SDL_AudioStream);

// SAFETY: SDL3 audio streams are thread-safe (every SDL_AudioStream call
// takes the stream's own lock); the pointer is only used while the
// `DeviceStream` that owns it is alive (see `DeviceStream`'s `Drop`).
unsafe impl Send for RawStream {}
unsafe impl Sync for RawStream {}

/// The owner of an SDL3 playback stream (main thread: SDL objects are
/// created and destroyed there). Keep it alive until the audio thread was
/// joined; if its [`DeviceSink`] still exists when it drops, the stream is
/// deliberately leaked instead of destroyed under the sink.
pub struct DeviceStream {
    owner: Option<sdl3::audio::AudioStreamOwner>,
    shared: Arc<RawStream>,
}

/// Queued audio a [`DeviceSink`] keeps in the device stream, in blocks
/// (about 21 ms at 48 kHz).
pub const DEVICE_QUEUE_BLOCKS: usize = 4;

impl DeviceStream {
    /// Opens the default playback device as `f32`, `sample_rate`, 7.1
    /// ([`CHANNELS`]) and starts it. Returns the owner (keep it on this
    /// thread) and the sink for the audio thread.
    ///
    /// # Errors
    /// SDL's message when no device can be opened.
    pub fn open(
        audio: &sdl3::AudioSubsystem,
        sample_rate: u32,
    ) -> Result<(DeviceStream, DeviceSink), String> {
        let spec = sdl3::audio::AudioSpec {
            freq: Some(i32::try_from(sample_rate).map_err(|e| e.to_string())?),
            channels: Some(CHANNELS as i32),
            format: Some(sdl3::audio::AudioFormat::f32_sys()),
        };
        let owner = audio
            .default_playback_device()
            .open_device_stream(Some(&spec))
            .map_err(|e| e.to_string())?;
        owner.resume().map_err(|e| e.to_string())?;
        let shared = Arc::new(RawStream(owner.raw()));
        let sink = DeviceSink {
            stream: Arc::clone(&shared),
            target_bytes: (DEVICE_QUEUE_BLOCKS * AUDIO_BLOCK_FRAMES * CHANNELS * 4) as i32,
        };
        Ok((
            DeviceStream {
                owner: Some(owner),
                shared,
            },
            sink,
        ))
    }
}

impl Drop for DeviceStream {
    fn drop(&mut self) {
        if Arc::strong_count(&self.shared) > 1 {
            // The audio thread may still write: never destroy the stream
            // under it (the OS reclaims it at exit).
            std::mem::forget(self.owner.take());
        }
    }
}

/// The audio-thread end of a [`DeviceStream`]: pushes blocks into the SDL
/// stream, paced to keep about [`DEVICE_QUEUE_BLOCKS`] blocks queued.
pub struct DeviceSink {
    stream: Arc<RawStream>,
    target_bytes: i32,
}

impl AudioSink for DeviceSink {
    fn name(&self) -> &'static str {
        "device"
    }

    fn paced(&self) -> bool {
        true
    }

    fn wants_block(&mut self) -> bool {
        // SAFETY: the stream is alive while this sink exists (DeviceStream
        // leaks it otherwise); SDL_GetAudioStreamQueued is thread-safe.
        let queued = unsafe { sdl3::sys::audio::SDL_GetAudioStreamQueued(self.stream.0) };
        queued >= 0 && queued < self.target_bytes
    }

    fn write(&mut self, block: &[f32], channels: usize) {
        if channels != CHANNELS {
            return;
        }
        let Ok(len) = i32::try_from(std::mem::size_of_val(block)) else {
            return;
        };
        // SAFETY: as in `wants_block`; `block` is valid for `len` bytes and
        // SDL copies it before returning. A failed put only drops this block.
        let _ = unsafe {
            sdl3::sys::audio::SDL_PutAudioStreamData(
                self.stream.0,
                block.as_ptr().cast::<core::ffi::c_void>(),
                len,
            )
        };
    }
}

/// Which telemetry element each simulation element is (SPEC §4 3.4,
/// `docs/TELEMETRY_PROTOCOL.md` §2.3): `(telemetry id, sim id)` pairs. The
/// telemetry id is the element's `number` in `table.toml` (what the DOF map
/// and every listener use); the sim id indexes the per-tick sources
/// ([`TelemetrySources`], `FrameStateSnapshot::lamps`). Elements not listed
/// stay off (0) on the wire.
#[derive(Clone, Debug, Default)]
pub struct TelemetryMap {
    /// `(switch number 0..=255, SwitchId)`.
    pub switches: Vec<(u8, u16)>,
    /// `(coil number 0..=255, CoilId)`.
    pub coils: Vec<(u8, u16)>,
    /// `(lamp number 0..=255, LampId)`: lamps whose kind is not `gi`.
    pub lamps: Vec<(u8, u16)>,
    /// `(GI string 0..=31, LampId)`: lamps with `kind = "gi"` (the GI id is
    /// the lamp's number); they are not sent as lamps.
    pub gi: Vec<(u8, u16)>,
    /// `(magnet number 0..=31, MagnetId)`.
    pub magnets: Vec<(u8, u16)>,
}

/// `(telemetry id, sim id)` when `number` fits `max` and the name resolves.
fn map_entry(number: u16, max: usize, id: Option<u16>) -> Option<(u8, u16)> {
    let n = u8::try_from(number)
        .ok()
        .filter(|&n| usize::from(n) <= max)?;
    Some((n, id?))
}

/// Sorts by telemetry id and keeps the first sim id of each.
fn sorted(mut v: Vec<(u8, u16)>) -> Vec<(u8, u16)> {
    v.sort_by_key(|&(n, _)| n);
    v.dedup_by_key(|&mut (n, _)| n);
    v
}

impl TelemetryMap {
    /// The map of `manifest` (load time; allocates): every declared switch
    /// (and every `[cabinet_switches]` input), coil, lamp, GI lamp and
    /// magnet whose `number` fits its telemetry range (switches, coils and
    /// lamps 0..=255; GI strings and magnets 0..=31) and whose name
    /// `id_of(kind, name)` resolves to its interned id. Out-of-range or
    /// unresolved elements are left out. Entries are sorted by telemetry
    /// id (the first element of a number wins).
    pub fn from_manifest(
        manifest: &TableManifest,
        id_of: impl Fn(IdKind, &str) -> Option<u16>,
    ) -> TelemetryMap {
        const MAX_ID: usize = 255;
        let mut switches: Vec<(u8, u16)> = manifest
            .switches
            .iter()
            .filter_map(|s| map_entry(s.number, MAX_ID, id_of(IdKind::Switch, &s.id)))
            .collect();
        for input in pin_core::input::CabinetInput::ALL {
            let n = manifest.cabinet_switches.number(input);
            switches.extend(map_entry(n, MAX_ID, id_of(IdKind::Switch, input.name())));
        }
        let coils = manifest
            .coils
            .iter()
            .filter_map(|c| map_entry(c.number, MAX_ID, id_of(IdKind::Coil, &c.id)))
            .collect();
        let lamp = |gi: bool, max: usize| {
            manifest
                .lamps
                .iter()
                .filter(|l| (l.kind == LampKind::Gi) == gi)
                .filter_map(|l| map_entry(l.number, max, id_of(IdKind::Lamp, &l.id)))
                .collect::<Vec<_>>()
        };
        let magnets = manifest
            .magnets
            .iter()
            .filter_map(|m| map_entry(m.number, MAGNET_COUNT - 1, id_of(IdKind::Magnet, &m.id)))
            .collect();
        TelemetryMap {
            switches: sorted(switches),
            coils: sorted(coils),
            lamps: sorted(lamp(false, MAX_ID)),
            gi: sorted(lamp(true, GI_COUNT - 1)),
            magnets: sorted(magnets),
        }
    }
}

/// The per-tick telemetry inputs that are not in the snapshot, indexed by
/// sim id (the second element of each [`TelemetryMap`] pair).
#[derive(Clone, Debug)]
pub struct TelemetrySources {
    /// Switch states: bit `i` (word `i / 64`, bit `i % 64`) = `SwitchId(i)`
    /// closed.
    pub switch_bits: [u64; 4],
    /// Coil states, indexed by `CoilId`: `true` = energised.
    pub coils: [bool; 256],
    /// Magnet power `0..=1`, indexed by `MagnetId`.
    pub magnets: [f32; 32],
}

impl Default for TelemetrySources {
    fn default() -> Self {
        TelemetrySources {
            switch_bits: [0; 4],
            coils: [false; 256],
            magnets: [0.0; 32],
        }
    }
}

/// A level as the protocol sends it (`TELEMETRY_PROTOCOL.md` §2.4): clamped
/// to `0..=1`, NaN and −0.0 as +0.0.
pub fn telemetry_level(v: f32) -> f32 {
    if v > 0.0 {
        // In (0, +inf]: `min` keeps it a non-NaN value in (0, 1].
        pin_core::fmath::min_f32(v, 1.0)
    } else {
        // NaN, negatives, -0.0 and +0.0.
        0.0
    }
}

/// Counters a [`TelemetryFeed`] shares with other threads (read them after
/// the feed moved to the sim thread).
#[derive(Clone, Debug, Default)]
pub struct TelemetryFeedStats {
    forced: Arc<AtomicU64>,
    ticks: Arc<AtomicU64>,
}

impl TelemetryFeedStats {
    /// Times a failed push forced a full state ([`TelemetryFeed::forced_full_states`]).
    pub fn forced_full_states(&self) -> u64 {
        self.forced.load(Ordering::Relaxed)
    }

    /// Ticks encoded so far.
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }
}

/// The feed's large buffers, boxed so a [`TelemetryFeed`] is small to move.
struct FeedBuffers {
    encoder: TelemetryEncoder,
    batch: DatagramBatch,
    state: TelemetryState,
}

/// The sim thread's telemetry encoder (SPEC §4 3.4, protocol §4): fills a
/// [`TelemetryState`] each tick through its [`TelemetryMap`], encodes it
/// (full state on the first tick, every 1000 ticks, after a tick going
/// backwards and after back-pressure; deltas otherwise) and pushes the
/// datagrams into the ring. Every buffer is allocated in
/// [`new`](Self::new); [`tick`](Self::tick) never allocates, locks or does
/// I/O (R4).
pub struct TelemetryFeed {
    buf: Box<FeedBuffers>,
    tx: TelemetryTx,
    map: TelemetryMap,
    stats: TelemetryFeedStats,
}

impl TelemetryFeed {
    /// A feed pushing into `tx` with `map` (load time; allocates). Its
    /// first tick sends the full state (protocol §4 rule 4, "on table
    /// load").
    pub fn new(tx: TelemetryTx, map: TelemetryMap) -> TelemetryFeed {
        TelemetryFeed {
            buf: Box::new(FeedBuffers {
                encoder: TelemetryEncoder::new(),
                batch: DatagramBatch::new(),
                state: TelemetryState::new(),
            }),
            tx,
            map,
            stats: TelemetryFeedStats::default(),
        }
    }

    /// The map.
    pub fn map(&self) -> &TelemetryMap {
        &self.map
    }

    /// One tick: fills the state (lamps and GI from `snapshot.lamps`,
    /// switches, coils and magnets from `sources`, each through the map;
    /// levels per [`telemetry_level`]), encodes it with this tick's
    /// `impulses` (kind 5, at most 64) and pushes the batch; a push that
    /// does not fit forces a full state on the next tick (protocol §4 rule
    /// 6). Never allocates, locks or does I/O.
    pub fn tick(
        &mut self,
        tick: u64,
        snapshot: &FrameStateSnapshot,
        sources: &TelemetrySources,
        impulses: &[HapticsTelemetryEvent],
    ) {
        let FeedBuffers {
            encoder,
            batch,
            state,
        } = &mut *self.buf;
        let m = &self.map;
        for &(n, id) in &m.switches {
            let i = usize::from(id);
            state.switches[usize::from(n)] = sources
                .switch_bits
                .get(i / 64)
                .is_some_and(|w| (w >> (i % 64)) & 1 == 1);
        }
        for &(n, id) in &m.coils {
            state.coils[usize::from(n)] = sources.coils.get(usize::from(id)) == Some(&true);
        }
        let level = |v: Option<&f32>| telemetry_level(v.copied().unwrap_or(0.0));
        for &(n, id) in &m.lamps {
            state.lamps[usize::from(n)] = level(snapshot.lamps.get(usize::from(id)));
        }
        for &(n, id) in &m.gi {
            if let Some(g) = state.gi.get_mut(usize::from(n)) {
                *g = level(snapshot.lamps.get(usize::from(id)));
            }
        }
        for &(n, id) in &m.magnets {
            if let Some(g) = state.magnets.get_mut(usize::from(n)) {
                *g = level(sources.magnets.get(usize::from(id)));
            }
        }
        encoder.encode_tick(tick, state, impulses, batch);
        if !self.tx.push_batch(batch) {
            encoder.force_full_state();
            self.stats.forced.fetch_add(1, Ordering::Relaxed);
        }
        self.stats.ticks.fetch_add(1, Ordering::Relaxed);
    }

    /// Times a failed push forced a full state.
    pub fn forced_full_states(&self) -> u64 {
        self.stats.forced_full_states()
    }

    /// Datagrams dropped on a full ring ([`TelemetryTx::dropped`]).
    pub fn ring_dropped(&self) -> u64 {
        self.tx.dropped()
    }

    /// Impulses beyond 64 in one tick that were not sent.
    pub fn dropped_impulses(&self) -> u64 {
        self.buf.encoder.dropped_impulses()
    }

    /// A handle on the feed's counters that stays readable after the feed
    /// moved to the sim thread.
    pub fn stats(&self) -> TelemetryFeedStats {
        self.stats.clone()
    }
}

/// What a [`TelemetryLink`] did, read when it stopped.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct TelemetryReport {
    /// Datagrams sent.
    pub sent: u64,
    /// Sends that failed (e.g. nothing listening on loopback).
    pub send_errors: u64,
    /// Datagrams the sim thread dropped on a full ring.
    pub ring_dropped: u64,
}

impl TelemetryReport {
    /// Adds `other`'s counts to these (one report over several engines).
    pub fn add(&mut self, other: TelemetryReport) {
        self.sent += other.sent;
        self.send_errors += other.send_errors;
        self.ring_dropped += other.ring_dropped;
    }
}

/// The running telemetry sender thread ([`open_telemetry`]).
pub struct TelemetryLink {
    handle: TelemetrySenderHandle,
    dest: SocketAddr,
}

impl TelemetryLink {
    /// Where the datagrams go.
    pub fn destination(&self) -> SocketAddr {
        self.dest
    }

    /// Stops the sender thread after a final pump (so stop it after the sim
    /// thread was joined: every queued datagram is sent) and joins it.
    pub fn stop(self) -> TelemetryReport {
        self.handle
            .stop()
            .map(|s| TelemetryReport {
                sent: s.sent(),
                send_errors: s.send_errors(),
                ring_dropped: s.dropped(),
            })
            .unwrap_or_default()
    }
}

/// Resolves a telemetry endpoint `"host:port"` (IPv4, `[IPv6]:port` or a
/// host name) to its first socket address.
///
/// # Errors
/// A message naming the endpoint and the reason.
pub fn parse_telemetry_endpoint(endpoint: &str) -> Result<SocketAddr, String> {
    endpoint
        .to_socket_addrs()
        .map_err(|e| format!("telemetry endpoint {endpoint:?}: {e}"))?
        .next()
        .ok_or_else(|| format!("telemetry endpoint {endpoint:?}: no address"))
}

/// How long the telemetry thread parks when the ring is empty: one tick, so
/// a datagram leaves at most about 1 ms after it was encoded (protocol §4
/// rule 7).
pub const TELEMETRY_POLL: Duration = Duration::from_millis(1);

/// Opens the telemetry path to `endpoint` (`"host:port"`): the datagram
/// ring ([`DEFAULT_TELEMETRY_RING_CAPACITY`]) and the sender thread
/// (`pin-telemetry`, [`TELEMETRY_POLL`]). Returns the sim thread's end of
/// the ring (give it to a [`TelemetryFeed`]) and the link.
///
/// # Errors
/// A bad endpoint, or a socket or thread that could not be created.
pub fn open_telemetry(endpoint: &str) -> Result<(TelemetryTx, TelemetryLink), String> {
    let dest = parse_telemetry_endpoint(endpoint)?;
    let (tx, rx) = telemetry_channel(DEFAULT_TELEMETRY_RING_CAPACITY);
    let sender = pin_audio::telemetry::TelemetrySender::new(rx, dest)
        .map_err(|e| format!("telemetry endpoint {endpoint:?}: socket: {e}"))?;
    let handle = sender
        .spawn(TELEMETRY_POLL)
        .map_err(|e| format!("telemetry: spawning the sender thread: {e}"))?;
    Ok((tx, TelemetryLink { handle, dest }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_audio::mixer::SoundTrigger;

    /// Builds the grey-box bundle (as `tests/support` and
    /// `tables/greybox/build.sh` do) into a fresh temporary directory.
    fn greybox_bundle(tag: &str) -> std::path::PathBuf {
        use pin_assetgen::{TableMeta, generate_kit};
        use pin_tablegen::layout::parse_layout;
        use pin_tablegen::{generate, write_bundle};
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/greybox");
        let out =
            std::env::temp_dir().join(format!("pin_app_audio_unit_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        std::fs::create_dir_all(&out).unwrap();
        let layout_text = std::fs::read_to_string(src.join("layout.toml")).unwrap();
        let table_text = std::fs::read_to_string(src.join("table.toml")).unwrap();
        let layout = parse_layout(&layout_text).unwrap();
        let base = TableManifest::from_toml_str(&table_text).unwrap();
        write_bundle(&generate(&layout, &base).unwrap(), &out).unwrap();
        for f in [
            "rules.lua",
            "backglass.toml",
            "layout.toml",
            "glass.toml",
            "display/scene_attract.toml",
            "display/scene_score.toml",
            "display/scene_game_over.toml",
            "display/scene_powerball_mania.toml",
        ] {
            let to = out.join(f);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::copy(src.join(f), &to).unwrap();
        }
        let hex = |c: &str| {
            let h = c.trim_start_matches('#');
            let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
            [b(0), b(2), b(4)]
        };
        let meta = TableMeta {
            title: base.meta.name.clone(),
            palette: base.meta.palette.iter().map(|c| hex(c)).collect(),
            seed: 0,
        };
        generate_kit(&meta, &out).unwrap();
        out
    }

    /// SPEC §4 4.4, D25: the I/O worker's loads use the budget last set
    /// with `IoWorker::set_decoded_budget_bytes` (default
    /// `DecodeBudget::DEFAULT_BYTES`), read when each load starts; and
    /// `load_app_table_with_budget` applies its budget directly.
    #[test]
    fn io_worker_loads_use_the_set_budget() {
        use super::super::table::{
            IoReply, IoRequest, IoWorker, load_app_table, load_app_table_with_budget,
        };
        let dir = greybox_bundle("worker");
        let mixer = MixerConfig::default();
        let w = IoWorker::spawn(mixer);
        assert_eq!(w.decoded_budget_bytes(), DecodeBudget::DEFAULT_BYTES);
        let sounds = |w: &IoWorker, id: u64| match w.recv() {
            Some(IoReply::Loaded { id: got, app, .. }) => {
                assert_eq!(got, id);
                app.samples.len()
            }
            other => panic!("{other:?}"),
        };
        w.request(IoRequest::Load {
            id: 1,
            path: dir.clone(),
        });
        let all = sounds(&w, 1);
        assert!(all >= 2);
        w.set_decoded_budget_bytes(0);
        assert_eq!(w.decoded_budget_bytes(), 0);
        w.request(IoRequest::Load {
            id: 2,
            path: dir.clone(),
        });
        assert_eq!(sounds(&w, 2), 0, "a zero budget leaves every sound silent");
        w.set_decoded_budget_bytes(DecodeBudget::DEFAULT_BYTES);
        w.request(IoRequest::Load {
            id: 3,
            path: dir.clone(),
        });
        assert_eq!(sounds(&w, 3), all);
        drop(w);
        assert_eq!(
            load_app_table_with_budget(&dir, mixer, 0)
                .unwrap()
                .samples
                .len(),
            0
        );
        assert_eq!(load_app_table(&dir, mixer).unwrap().samples.len(), all);
    }

    /// SPEC §4 3.3 / 4.4, D25: `table_samples` with the default budget
    /// (`DecodeBudget::DEFAULT_BYTES`, 512 MiB) loads every grey-box sound
    /// with no errors; with a budget below the bundle's decoded size, every
    /// sound that no longer fits fails with exactly one budget message
    /// naming the budget in MiB and the setting, stays silent, and the load
    /// goes on (earlier sounds keep their charge).
    #[test]
    fn table_samples_uses_the_given_budget() {
        let dir = greybox_bundle("budget");
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(move || {
                let checked = super::super::table::validate_table(&dir).unwrap();
                let rate = MixerConfig::default().sample_rate;
                let manifest_len = checked.table.manifest.sounds.len();
                let (all, errors) = table_samples(&checked, rate, DecodeBudget::DEFAULT_BYTES);
                assert!(errors.is_empty(), "{errors:?}");
                assert_eq!(all.len(), manifest_len);
                assert!(manifest_len >= 2);
                // A zero budget: every sound fails with the budget message.
                let (none, errors) = table_samples(&checked, rate, 0);
                assert!(none.is_empty());
                assert_eq!(errors.len(), manifest_len);
                for (e, def) in errors.iter().zip(&checked.table.manifest.sounds) {
                    assert_eq!(
                        e,
                        &format!(
                            "sound {} ({}): decoded-audio budget of 0 MiB exhausted; \
                             raise audio.decoded_budget_mib",
                            def.id, def.file
                        )
                    );
                }
                // Room for exactly the first manifest sound: it loads, the
                // rest fail with "1 MiB" when the budget prints as 1 MiB.
                let first = checked.table.ids.sound(&checked.table.manifest.sounds[0].id);
                let first_bytes = all
                    .iter()
                    .find(|(id, _, _)| Some(*id) == first)
                    .map(|(_, s, _)| s.frames() as u64 * pin_audio::mixer::BYTES_PER_FRAME)
                    .unwrap();
                let budget = (1 << 20).max(first_bytes);
                let (some, errors) = table_samples(&checked, rate, budget);
                let fit: u64 = some
                    .iter()
                    .map(|(_, s, _)| s.frames() as u64 * pin_audio::mixer::BYTES_PER_FRAME)
                    .sum();
                assert!(fit <= budget);
                assert_eq!(some.len() + errors.len(), manifest_len);
                for e in &errors {
                    assert!(
                        e.ends_with(&format!(
                            "decoded-audio budget of {} MiB exhausted; raise audio.decoded_budget_mib",
                            budget >> 20
                        )),
                        "{e}"
                    );
                }
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn null_and_offline_sinks() {
        let mut n = NullSink::default();
        n.write(&[0.0; 16], 8);
        assert_eq!(n.frames(), 2);
        assert!(!n.paced() && n.wants_block());
        let (mut s, rec) = OfflineSink::new(3);
        s.write(&[0.5; 16], CHANNELS);
        s.write(&[0.25; 16], CHANNELS);
        s.write(&[1.0; 6], 2); // other layouts are ignored
        let got = rec.samples();
        assert_eq!(got.len(), 24);
        assert_eq!(got[23], 0.25);
        assert_eq!(rec.peak(), 0.5);
    }

    #[test]
    fn engine_plays_a_triggered_sound_on_the_front_channels() {
        let (mut tx, rx) = trigger_ring(4);
        let s = Arc::new(Sample::constant(1000, 0.5, -0.5));
        let mut e = AudioEngine::new(
            SsfConfig::default(),
            MixerConfig::default(),
            Some(rx),
            &[(SoundId(3), s, Bus::Sfx)],
        );
        assert!(e.render_block().iter().all(|&x| x == 0.0), "silent at rest");
        assert!(tx.send(SoundTrigger {
            sound: SoundId(3),
            gain: 1.0
        }));
        let b = e.render_block().to_vec();
        assert_eq!(e.peak_voices(), 1);
        assert_eq!(b[pin_audio::ssf::CH_FL], 0.5);
        assert_eq!(b[pin_audio::ssf::CH_FR], -0.5);
        assert_eq!(e.mixer().frames_rendered(), 2 * AUDIO_BLOCK_FRAMES as u64);
    }

    /// Energy on the left (SL + BL) and right (SR + BR) exciters of a few
    /// blocks after one impulse from `source`.
    fn exciter_energy(positions: &[(u16, [f32; 2])], source: u16) -> (f64, f64) {
        use pin_audio::ssf::{CH_BL, CH_BR, CH_SL, CH_SR};
        let mut e = AudioEngine::new(SsfConfig::default(), MixerConfig::default(), None, &[])
            .with_source_positions(positions);
        e.feed_haptic(HapticEvent::from(pin_core::input::HapticsTelemetryEvent {
            tick: 1,
            source,
            strength: 1.0,
        }));
        let (mut l, mut r) = (0.0f64, 0.0f64);
        for _ in 0..8 {
            for f in e.render_block().chunks_exact(CHANNELS) {
                let sq = |c: usize| f64::from(f[c]) * f64::from(f[c]);
                l += sq(CH_SL) + sq(CH_BL);
                r += sq(CH_SR) + sq(CH_BR);
            }
        }
        (l, r)
    }

    /// PR #21 review (audio.rs:101): registered source positions pan the
    /// impulses; a left source is louder on the left exciters than a right
    /// one, and mirrored.
    #[test]
    fn source_positions_pan_impulses_left_and_right() {
        let positions = [(4u16, [-200.0f32, 150.0]), (5, [200.0, 150.0])];
        let (l4, r4) = exciter_energy(&positions, 4);
        let (l5, r5) = exciter_energy(&positions, 5);
        assert!(l4 > 0.0 && r5 > 0.0, "silent: {l4} {r5}");
        assert!(
            l4 > l5 * 4.0,
            "left source not louder on the left: {l4} vs {l5}"
        );
        assert!(
            r5 > r4 * 4.0,
            "right source not louder on the right: {r5} vs {r4}"
        );
        assert!(l4 > r4 && r5 > l5);
        // Mirrored positions give mirrored energy.
        assert!((l4 - r5).abs() <= 1e-6 * l4, "{l4} vs {r5}");
        assert!((r4 - l5).abs() <= 1e-6 * l4.max(1e-12), "{r4} vs {l5}");
        // Without positions both sources play at the centre: identical.
        let (lc4, rc4) = exciter_energy(&[], 4);
        let (lc5, rc5) = exciter_energy(&[], 5);
        assert_eq!((lc4, rc4), (lc5, rc5));
        assert!((lc4 - rc4).abs() <= 1e-6 * lc4);
    }

    #[test]
    fn playfield_rect_follows_the_table_bounds() {
        assert_eq!(
            ssf_playfield_mm(([0.0, 0.0, 0.0], [0.514, 1.07, 0.1])),
            [0.0, 514.0, 0.0, 1070.0]
        );
        let default = SsfConfig::default().playfield_mm;
        assert_eq!(ssf_playfield_mm(([0.0; 3], [0.0; 3])), default);
        assert_eq!(
            ssf_playfield_mm(([f32::NAN, 0.0, 0.0], [1.0, 1.0, 0.0])),
            default
        );
    }

    mod telemetry {
        use super::super::*;
        use pin_audio::telemetry::{TelemetryKind, decode_datagram, telemetry_channel};
        use pin_core::schema::table::MagnetDef;
        use proptest::prelude::*;

        const TABLE_TOML: &str =
            include_str!("../../../../tools/dof_bridge/tests/fixtures/table.toml");

        /// The fixture table with a magnet and a GI lamp above 31.
        fn manifest() -> TableManifest {
            let mut m = TableManifest::from_toml_str(TABLE_TOML).unwrap();
            m.magnets.push(MagnetDef {
                id: "mag".into(),
                number: 3,
                position: [0.0; 3],
                k_coil: 1.0,
                d_min: 1.0,
                range: 10.0,
                tau_ms: None,
            });
            m.magnets.push(MagnetDef {
                id: "mag_high".into(),
                number: 40,
                ..m.magnets[0].clone()
            });
            let mut gi = m.lamps.iter().find(|l| l.id == "gi_upper").unwrap().clone();
            gi.id = "gi_high".into();
            gi.number = 32;
            m.lamps.push(gi);
            m
        }

        /// Interned ids deliberately different from the numbers: id =
        /// number + 100 (except unresolved names), so a mix-up shows.
        fn shifted(m: &TableManifest) -> TelemetryMap {
            TelemetryMap::from_manifest(m, |kind, name| {
                let n = match kind {
                    IdKind::Switch => m
                        .switches
                        .iter()
                        .find(|s| s.id == name)
                        .map(|s| s.number)
                        .or_else(|| {
                            pin_core::input::CabinetInput::ALL
                                .into_iter()
                                .find(|i| i.name() == name)
                                .map(|i| m.cabinet_switches.number(i))
                        }),
                    IdKind::Coil => m.coils.iter().find(|c| c.id == name).map(|c| c.number),
                    IdKind::Lamp => m.lamps.iter().find(|l| l.id == name).map(|l| l.number),
                    IdKind::Magnet => m.magnets.iter().find(|l| l.id == name).map(|l| l.number),
                    _ => None,
                };
                n.filter(|_| name != "trough_eject").map(|n| n + 100)
            })
        }

        /// KP-006 (SPEC §4 3.4): the map pairs table numbers (telemetry ids)
        /// with interned ids; GI lamps are GI strings, never lamps; numbers
        /// out of a kind's range and unresolved names are left out.
        #[test]
        fn kp006_map_uses_numbers_on_the_wire_and_ids_inside() {
            let map = shifted(&manifest());
            assert_eq!(
                map.switches,
                [1, 2, 3, 4, 5, 6, 7, 8, 21, 30, 50].map(|n| (n, u16::from(n) + 100))
            );
            assert_eq!(map.coils, [1, 2, 9, 12].map(|n| (n, u16::from(n) + 100)));
            assert_eq!(
                map.lamps,
                [40, 41, 60, 100].map(|n| (n, u16::from(n) + 100))
            );
            assert_eq!(map.gi, [(0, 100), (2, 102)], "gi_high (32) left out");
            assert_eq!(map.magnets, [(3, 103)], "magnet 40 left out");
        }

        /// KP-006: the feed reads each element through the map; unmapped
        /// and out-of-range sim ids stay off.
        #[test]
        fn kp006_feed_fills_the_state_through_the_map() {
            let (tx, mut rx) = telemetry_channel(64);
            let map = TelemetryMap {
                switches: vec![(10, 3), (11, 300)],
                coils: vec![(20, 7), (21, 999)],
                lamps: vec![(30, 5), (31, 900)],
                gi: vec![(1, 6), (40, 6)],
                magnets: vec![(2, 1), (50, 1), (3, 77)],
            };
            let mut feed = TelemetryFeed::new(tx, map);
            assert_eq!(feed.map().lamps.len(), 2);
            let mut snap = FrameStateSnapshot::default();
            snap.lamps[5] = 0.25;
            snap.lamps[6] = 2.0;
            let mut src = TelemetrySources::default();
            src.switch_bits[0] = 1 << 3;
            src.coils[7] = true;
            src.magnets[1] = -0.0;
            let imp = [HapticsTelemetryEvent {
                tick: 0,
                source: 0x1003,
                strength: 0.5,
            }];
            feed.tick(0, &snap, &src, &imp);
            let mut entries = Vec::new();
            while let Some(d) = rx.pop() {
                let (h, e) = decode_datagram(d.as_bytes()).unwrap();
                assert!(h.is_full_state());
                entries.extend(e);
            }
            assert_eq!(entries.len(), 832 + 1);
            let value = |k: TelemetryKind, id: u16| {
                entries
                    .iter()
                    .find(|e| e.kind == k && e.id == id)
                    .map(|e| e.value)
                    .unwrap()
            };
            assert_eq!(value(TelemetryKind::Switch, 10), 1.0);
            assert_eq!(value(TelemetryKind::Switch, 11), 0.0);
            assert_eq!(value(TelemetryKind::Coil, 20), 1.0);
            assert_eq!(value(TelemetryKind::Coil, 21), 0.0);
            assert_eq!(value(TelemetryKind::Lamp, 30), 0.25);
            assert_eq!(value(TelemetryKind::Lamp, 31), 0.0);
            assert_eq!(value(TelemetryKind::Gi, 1), 1.0);
            assert_eq!(value(TelemetryKind::Magnet, 2).to_bits(), 0);
            assert_eq!(value(TelemetryKind::Magnet, 3), 0.0);
            assert_eq!(entries.last().unwrap().kind, TelemetryKind::HapticImpulse);
            // Next tick, nothing changed: no datagram.
            feed.tick(1, &snap, &src, &[]);
            assert!(rx.pop().is_none());
            snap.lamps[5] = 0.5;
            feed.tick(2, &snap, &src, &[]);
            let (h, e) = decode_datagram(rx.pop().unwrap().as_bytes()).unwrap();
            assert!(!h.is_full_state());
            assert_eq!((e.len(), e[0].id, e[0].value), (1, 30, 0.5));
            assert_eq!((feed.forced_full_states(), feed.ring_dropped()), (0, 0));
            assert_eq!(feed.dropped_impulses(), 0);
            assert_eq!(feed.stats().ticks(), 3);
        }

        /// KP-006 (protocol §4 rule 6): a push that does not fit forces a
        /// full state on the next tick.
        #[test]
        fn kp006_back_pressure_forces_a_full_state() {
            let (tx, mut rx) = telemetry_channel(5);
            let mut feed = TelemetryFeed::new(
                tx,
                TelemetryMap {
                    lamps: vec![(0, 0)],
                    ..TelemetryMap::default()
                },
            );
            let stats = feed.stats();
            let mut snap = FrameStateSnapshot::default();
            let src = TelemetrySources::default();
            feed.tick(0, &snap, &src, &[]); // full state: 5 datagrams, ring full
            snap.lamps[0] = 1.0;
            feed.tick(1, &snap, &src, &[]); // does not fit
            assert_eq!((feed.forced_full_states(), feed.ring_dropped()), (1, 1));
            assert_eq!(stats.forced_full_states(), 1);
            while rx.pop().is_some() {}
            feed.tick(2, &snap, &src, &[]);
            let (h, _) = decode_datagram(rx.pop().unwrap().as_bytes()).unwrap();
            assert!(h.is_full_state() && h.tick == 2);
        }

        /// KP-006: a bad endpoint is an error naming it; a good one opens
        /// the sender thread, which stops cleanly.
        #[test]
        fn kp006_open_telemetry_endpoints() {
            for bad in ["", "no-port", "127.0.0.1", "127.0.0.1:notaport", "[::1"] {
                let e = open_telemetry(bad).err().expect(bad);
                assert!(e.starts_with("telemetry endpoint "), "{e}");
                assert!(parse_telemetry_endpoint(bad).is_err());
            }
            assert_eq!(
                parse_telemetry_endpoint("[::1]:9").unwrap(),
                "[::1]:9".parse::<SocketAddr>().unwrap()
            );
            let (_tx, link) = open_telemetry("127.0.0.1:9").unwrap();
            assert_eq!(link.destination(), "127.0.0.1:9".parse().unwrap());
            assert_eq!(link.stop(), TelemetryReport::default());
            let mut r = TelemetryReport {
                sent: 1,
                send_errors: 2,
                ring_dropped: 3,
            };
            r.add(r);
            assert_eq!((r.sent, r.send_errors, r.ring_dropped), (2, 4, 6));
        }

        #[test]
        fn kp006_level_edge_cases() {
            assert_eq!(telemetry_level(f32::NAN).to_bits(), 0);
            assert_eq!(telemetry_level(-0.0).to_bits(), 0);
            assert_eq!(telemetry_level(f32::INFINITY), 1.0);
            assert_eq!(telemetry_level(f32::NEG_INFINITY), 0.0);
            assert_eq!(telemetry_level(0.5), 0.5);
        }

        proptest! {
            /// KP-006 (protocol §2.4): every level is in [0, 1], never NaN
            /// or -0.0, and values inside [0, 1] pass unchanged.
            #[test]
            fn kp006_level_is_clamped_and_canonical(bits in any::<u32>()) {
                let v = f32::from_bits(bits);
                let l = telemetry_level(v);
                prop_assert!((0.0..=1.0).contains(&l));
                prop_assert!(l.to_bits() != (-0.0f32).to_bits());
                if v > 0.0 && v <= 1.0 {
                    prop_assert_eq!(l.to_bits(), v.to_bits());
                }
            }
        }
    }
}
