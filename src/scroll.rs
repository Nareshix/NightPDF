
use std::collections::VecDeque;
use std::time::Instant;

const DECEL_FRICTION: f64 = 4.0;
const OVERSHOOT_FRICTION: f64 = 20.0;
const MAX_OVERSHOOT: f64 = 100.0;
pub const MAGIC_SCROLL_FACTOR: f64 = 2.5;
const VELOCITY_ACCUMULATION_FLOOR: f64 = 0.33;
const VELOCITY_ACCUMULATION_CEIL: f64 = 1.0;
const VELOCITY_ACCUMULATION_MAX: f64 = 6.0;

#[derive(PartialEq)]
enum Phase {
    Decelerating,
    Overshooting,
    Finished,
}

pub struct KineticScrolling {
    phase: Phase,
    lower: f64,
    upper: f64,
    c1: f64,
    c2: f64,
    equilibrium: f64,
    t: Instant,
    pub position: f64,
    pub velocity: f64,
}

impl KineticScrolling {
    pub fn new(lower: f64, upper: f64, pos: f64, vel: f64) -> Self {
        let mut s = Self {
            phase: Phase::Decelerating,
            lower,
            upper,
            c1: vel / DECEL_FRICTION + pos,
            c2: -vel / DECEL_FRICTION,
            equilibrium: 0.0,
            t: Instant::now(),
            position: pos,
            velocity: vel,
        };
        if pos < lower {
            s.init_overshoot(lower, pos, vel);
        } else if pos > upper {
            s.init_overshoot(upper, pos, vel);
        }
        s
    }

    fn init_overshoot(&mut self, eq: f64, pos: f64, vel: f64) {
        self.phase = Phase::Overshooting;
        self.equilibrium = eq;
        self.c1 = pos - eq;
        self.c2 = vel + OVERSHOOT_FRICTION / 2.0 * self.c1;
        self.t = Instant::now();
    }

    pub fn tick(&mut self) -> (f64, bool) {
        let t = self.t.elapsed().as_secs_f64();

        match self.phase {
            Phase::Decelerating => {
                let e = (-DECEL_FRICTION * t).exp();
                self.position = self.c1 + self.c2 * e;
                self.velocity = -DECEL_FRICTION * self.c2 * e;

                if self.position < self.lower {
                    self.init_overshoot(self.lower, self.position, self.velocity);
                } else if self.position > self.upper {
                    self.init_overshoot(self.upper, self.position, self.velocity);
                } else if self.velocity.abs() < 0.1 {
                    self.phase = Phase::Finished;
                    self.position = self.position.round();
                }
            }
            Phase::Overshooting => {
                let half = MAX_OVERSHOOT / 2.0;
                let e = (-OVERSHOOT_FRICTION / 2.0 * t).exp();
                let mut pos = e * (self.c1 + self.c2 * t);

                if pos < self.lower - half || pos > self.upper + half {
                    pos = pos.clamp(self.lower - half, self.upper + half);
                    self.init_overshoot(self.equilibrium, pos, 0.0);
                } else {
                    self.velocity = self.c2 * e - OVERSHOOT_FRICTION / 2.0 * pos;
                }

                self.position = pos + self.equilibrium;

                if pos.abs() < 0.1 {
                    self.phase = Phase::Finished;
                    self.position = self.equilibrium;
                    self.velocity = 0.0;
                }
            }
            Phase::Finished => {}
        }

        (self.position, self.phase != Phase::Finished)
    }

    pub fn stop(&mut self) {
        if self.phase == Phase::Decelerating {
            self.phase = Phase::Finished;
            self.position = self.position.round();
        }
    }
}

pub fn accumulate_velocity(kinetic: &mut Option<KineticScrolling>, velocity: &mut f64) {
    let Some(k) = kinetic else { return };

    let last_velocity = k.velocity;
    let same_direction = (*velocity >= 0.0) == (last_velocity >= 0.0);
    let above_floor = velocity.abs() >= last_velocity.abs() * VELOCITY_ACCUMULATION_FLOOR;

    if same_direction && above_floor {
        let min_vel = last_velocity * VELOCITY_ACCUMULATION_FLOOR;
        let max_vel = last_velocity * VELOCITY_ACCUMULATION_CEIL;
        let range = max_vel - min_vel;
        if range.abs() > f64::EPSILON {
            let mult = (*velocity - min_vel) / range;
            *velocity += last_velocity * mult.min(VELOCITY_ACCUMULATION_MAX);
        }
    }
}

pub fn wheel_detent_step(page_size: f64) -> f64 {
    page_size.powf(2.0 / 3.0)
}

pub struct VelocityTracker {
    history: VecDeque<(Instant, f64)>,
}

impl VelocityTracker {
    pub fn new() -> Self {
        Self {
            history: VecDeque::new(),
        }
    }

    pub fn push(&mut self, delta: f64) {
        let now = Instant::now();
        self.history
            .retain(|(t, _)| now.duration_since(*t).as_millis() < 100);
        self.history.push_back((now, delta));
    }

    pub fn velocity(&self) -> f64 {
        if self.history.len() < 2 {
            return 0.0;
        }
        let total: f64 = self.history.iter().map(|(_, d)| d).sum();
        let span = self.history.front().unwrap().0.elapsed().as_secs_f64();
        if span > 0.0 {
            total / span
        } else {
            0.0
        }
    }

    pub fn clear(&mut self) {
        self.history.clear();
    }
}