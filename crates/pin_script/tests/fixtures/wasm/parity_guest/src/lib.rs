//! Rust twin of `../rules.lua` (Subphase 4.5 parity fixture), built to
//! `../rules.wasm` by `../rebuild.sh`. Keep the two in lockstep: every
//! host call, its order and its arguments must match.

#![no_std]

use pin_guest_sdk::{self as pin, Rules};

struct Parity {
    score: [i64; 4],
    player: i64,
    combo: i64,
    held: bool,
    lamp_phase: i64,
    hits: i64,
    jackpot: u16,
}

impl Parity {
    fn slot(&mut self) -> &mut i64 {
        &mut self.score[((self.player - 1) & 3) as usize]
    }
}

impl Rules for Parity {
    fn on_init(&mut self) {
        self.jackpot = pin::sound("jackpot").unwrap_or(0);
        pin::show_scene(1);
        pin::persist_set(0, pin::persist_get(0) + 1);
        pin::spawn_ball(0, 0);
        pin::set_gi_tint(1.0, 1.0, 1.0, 1.0);
    }

    fn on_switch_edge(&mut self, sw: u16, closed: bool) {
        let id = i64::from(sw);
        if closed {
            self.hits += 1;
            let mut pts = 10 * (id + 1) + pin::random(0, 9);
            if self.combo > 0 {
                pts *= 2;
            }
            self.combo = 250;
            let player = self.player as u8;
            let slot = self.slot();
            *slot += pts;
            pin::set_score(player, *slot);
            if id % 4 == 0 {
                pin::pulse_coil(sw, (10 + id % 20) as u16);
            }
            if id % 5 == 1 {
                pin::play_sound((id % 8) as u16, 1.0);
            }
            if id == 7 {
                self.held = !self.held;
                pin::hold_coil(2, self.held);
            }
            if self.hits % 25 == 0 {
                let profile = pin::random(1, 3);
                pin::spawn_ball((self.hits % 3) as u16, profile as u16);
                pin::show_scene((2 + self.hits % 4) as u16);
            }
            if self.hits % 17 == 0 {
                self.player = self.player % 4 + 1;
                pin::set_diverter(1, self.player % 2 == 0);
            }
        } else {
            if id == 3 {
                pin::set_zipper(0, false);
            }
            if id == 4 {
                pin::set_zipper(0, true);
            }
        }
    }

    fn on_tick(&mut self) {
        let t = pin::tick();
        if self.combo > 0 {
            self.combo -= 1;
        }
        if t % 50 == 0 {
            self.lamp_phase = (self.lamp_phase + 1) % 8;
            for i in 0..8 {
                let on = if i == self.lamp_phase { 1.0 } else { 0.0 };
                pin::set_lamp(i as u16, on);
            }
        }
        if t % 97 == 0 {
            let r = pin::random_to(100);
            if r > 70 {
                pin::play_sound(self.jackpot, 1.0);
            }
            pin::set_magnet(0, if r < 10 { 1.0 } else { 0.0 });
        }
        if t % 500 == 0 {
            let best = pin::persist_get(1);
            let s = *self.slot();
            if s > best {
                pin::persist_set(1, s);
            }
            pin::persist_set(2, pin::persist_get(2) + i64::from(pin::balls_in_play()));
        }
        if pin::switch(5) && t % 10 == 0 {
            pin::pulse_coil(5, 15);
        }
        if t % 333 == 0 {
            pin::set_gi_tint(1.0, 0.0, (t % 2) as f32, 1.0);
        }
    }
}

pin_guest_sdk::export_rules!(Parity = Parity {
    score: [0; 4],
    player: 1,
    combo: 0,
    held: false,
    lamp_phase: 0,
    hits: 0,
    jackpot: 0,
});

#[cfg(target_arch = "wasm32")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
