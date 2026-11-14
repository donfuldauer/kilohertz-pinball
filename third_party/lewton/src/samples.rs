// Vorbis decoder written in Rust
//
// Copyright (c) 2019 est31 <MTest31@outlook.com>
// and contributors. All rights reserved.
// Licensed under MIT license, or Apache 2 license,
// at your option. Please see the LICENSE file
// attached to this source distribution for details.

/*!
Traits for sample formats
*/

/// Trait for a packet of multiple samples
pub trait Samples {
	fn num_samples(&self) -> usize;
	fn truncate(&mut self, limit :usize);
	fn from_floats(floats :Vec<Vec<f32>>) -> Self;
}

impl<S :Sample> Samples for Vec<Vec<S>> {
	fn num_samples(&self) -> usize {
		self[0].len()
	}
	fn truncate(&mut self, limit :usize) {
		for ch in self.iter_mut() {
			if limit < ch.len() {
				ch.truncate(limit);
			}
		}
	}

	fn from_floats(floats :Vec<Vec<f32>>) -> Self {
		floats.into_iter()
			.map(|samples| {
				samples.into_iter()
					.map(S::from_float)
					.collect()
			}).collect()
	}
}

/// A packet of multi-channel interleaved samples
pub struct InterleavedSamples<S :Sample> {
	pub samples :Vec<S>,
	pub channel_count :usize,
}

impl<S :Sample> Samples for InterleavedSamples<S> {
	fn num_samples(&self) -> usize {
		self.samples.len() / self.channel_count
	}
	fn truncate(&mut self, limit :usize) {
		// KHZ patch 5: clamp first, so `limit * channel_count` cannot overflow.
		let limit = std::cmp::min(limit, self.num_samples());
		self.samples.truncate(limit * self.channel_count);
	}
	fn from_floats(floats :Vec<Vec<f32>>) -> Self {
		let channel_count = floats.len();
		// Note that a channel count of 0 is forbidden
		// by the spec and the header decoding code already
		// checks for that.
		assert!(floats.len() > 0);
		let samples_interleaved = if channel_count == 1 {
			// Because decoded_pck[0] doesn't work...
			<Vec<Vec<S>> as Samples>::from_floats(floats).into_iter().next().unwrap()
		} else {
			let len = floats[0].len();
			let mut samples = Vec::with_capacity(len * channel_count);
			for i in 0 .. len {
				for ref chan in floats.iter() {
					samples.push(S::from_float(chan[i]));
				}
			}
			samples
		};
		Self {
			samples : samples_interleaved,
			channel_count,
		}
	}
}

/// Trait representing a single sample
pub trait Sample {
	fn from_float(fl :f32) -> Self;
}

impl Sample for f32 {
	fn from_float(fl :f32) -> Self {
		fl
	}
}

impl Sample for i16 {
	fn from_float(fl :f32) -> Self {
		let fl = fl * 32768.0;
		if fl > 32767. {
			32767
		} else if fl < -32768. {
			-32768
		} else {
			fl as i16
		}
	}
}

#[test]
fn khz_interleaved_truncate_clamps_before_multiplying() {
	// KHZ patch 5: `limit * channel_count` wrapped to 0 (or panicked).
	let mut s = InterleavedSamples::<i16> { samples : vec![1, 2, 3, 4, 5, 6], channel_count : 2 };
	s.truncate(::std::usize::MAX / 2 + 1);
	assert_eq!(s.samples.len(), 6);
	s.truncate(2);
	assert_eq!(s.samples, vec![1, 2, 3, 4]);
}
